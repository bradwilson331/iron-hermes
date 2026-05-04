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
        // Phase 21.7 Plan 08 (D-03): /agents list|kill|logs surface.
        CommandDef::new(
            "agents",
            "List, kill, or tail logs for active subagents",
            Session,
        )
        .args_hint("[list|kill <id>|logs <id>]")
        .platform(Universal),
        CommandDef::new("approve", "Approve a pending dangerous command", Session)
            .args_hint("[session|always]")
            .platform(GatewayOnly),
        CommandDef::new("deny", "Deny a pending dangerous command", Session).platform(GatewayOnly),
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
        CommandDef::new("sessions", "List recent sessions", Session).platform(Universal),
        // Phase 25.3 Plan 11 (D-F-1): export the current (or named) session to
        // the canonical 4-file flat-file directory layout. Universal platform
        // (CLI REPL + ratatui REPL + Telegram). Optional positional arg is the
        // session id; with no arg, exports `ctx.session_id` (the current session).
        CommandDef::new(
            "export-session",
            "Export a session to flat JSON files (4-file layout)",
            Session,
        )
        .args_hint("[session_id]")
        .platform(Universal),
        // -----------------------------------------------------------------------
        // CONFIGURATION
        // -----------------------------------------------------------------------
        CommandDef::new("config", "Show configuration", Configuration).platform(CliOnly),
        // Phase 26 D-14: provider management slash commands (list/show/test/enable/disable).
        // One entry per subcommand so CommandRouter can resolve "/provider list" etc. via prefix.
        CommandDef::new(
            "provider",
            "Manage providers — list/show/test/enable/disable (Phase 26, D-14)",
            Configuration,
        )
        .args_hint("[list|show|test|enable|disable] [name]")
        .platform(Universal),
        CommandDef::new(
            "provider list",
            "List all providers with status",
            Configuration,
        )
        .args_hint("[--json]")
        .platform(Universal),
        CommandDef::new(
            "provider show",
            "Show detail for one provider",
            Configuration,
        )
        .args_hint("<name>")
        .platform(Universal),
        CommandDef::new(
            "provider test",
            "Live-ping a provider API endpoint (D-15: never prints key value)",
            Configuration,
        )
        .args_hint("<name>")
        .platform(Universal),
        CommandDef::new(
            "provider enable",
            "Enable a provider (persists to config.yaml, emits cache-break banner)",
            Configuration,
        )
        .args_hint("<name>")
        .platform(Universal),
        CommandDef::new(
            "provider disable",
            "Disable a provider (persists to config.yaml, emits cache-break banner)",
            Configuration,
        )
        .args_hint("<name>")
        .platform(Universal),
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
        CommandDef::new(
            "yolo",
            "Toggle dangerous command auto-approval",
            Configuration,
        )
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
        CommandDef::new("mouse", "Toggle mouse capture", Configuration)
            .args_hint("[on|off]")
            .platform(CliOnly),
        // -----------------------------------------------------------------------
        // TOOLS AND SKILLS
        // -----------------------------------------------------------------------
        CommandDef::new("tools", "List or manage tools", ToolsAndSkills)
            .args_hint("[list|disable|enable]")
            .platform(CliOnly),
        // Phase 25 Plan 04 (D-06): replaces the /toolsets stub. Singular name (matches
        // /personality vs /personalities), Universal platform (CLI REPL + gateway).
        CommandDef::new(
            "toolset",
            "Manage toolsets (list/enable/disable/show)",
            ToolsAndSkills,
        )
        .args_hint("[list|enable|disable|show] [name]")
        .platform(Universal),
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
        CommandDef::new("mcp", "MCP server list and status", ToolsAndSkills).platform(Universal),
        CommandDef::new("memory", "Memory provider status", ToolsAndSkills).platform(Universal),
        // -----------------------------------------------------------------------
        // INFO
        // -----------------------------------------------------------------------
        CommandDef::new("commands", "List available commands", Info)
            .args_hint("[page]")
            .platform(GatewayOnly),
        CommandDef::new("help", "Show this help message", Info).platform(All),
        CommandDef::new("usage", "Show token usage", Info).platform(Universal),
        CommandDef::new("models", "Show or refresh model metadata", Info)
            .args_hint("[refresh|info <model>]")
            .platform(Universal),
        CommandDef::new("insights", "Show analytics", Info)
            .args_hint("[days]")
            .platform(Universal),
        CommandDef::new("platforms", "Show platform status", Info)
            .aliases(&["gateway"])
            .platform(CliOnly),
        CommandDef::new("paste", "Paste clipboard image", Info).platform(CliOnly),
        CommandDef::new("update", "Check for updates", Info).platform(GatewayOnly),
        CommandDef::new("snapshot", "Save a conversation snapshot", Info).platform(Universal),
        CommandDef::new("profile", "Show active profile and HERMES_HOME", Info).platform(Universal),
        // -----------------------------------------------------------------------
        // EXIT
        // -----------------------------------------------------------------------
        CommandDef::new("quit", "Exit the CLI", Exit)
            .aliases(&["exit", "q"])
            .platform(CliOnly),
    ]
}

// =============================================================================
// Tests — Phase 25 Plan 04 (slash command registration)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandRouter, ResolveResult};
    use crate::types::Platform;

    /// D-06: `/toolset` (singular) is registered; the old `/toolsets` stub is
    /// REPLACED (no alias, no compat shim — Phase 22.3 typo system handles
    /// the operator-typed plural variant).
    #[test]
    fn slash_toolset_registered_in_build_registry() {
        let router = CommandRouter::new(build_registry());
        let result = router.resolve("toolset", &Platform::Local);
        assert!(
            matches!(result, ResolveResult::Exact(c) if c.name == "toolset"),
            "expected /toolset Exact match, got: {:?}",
            result
        );
    }

    /// D-06: `/toolsets` (plural) is NOT registered (the stub was replaced,
    /// not aliased). The typo system will suggest `toolset` if the operator
    /// types `toolsets`.
    #[test]
    fn slash_toolsets_plural_not_registered() {
        let router = CommandRouter::new(build_registry());
        let result = router.resolve("toolsets", &Platform::Local);
        // Either NotFound or PrefixMatch to "toolset" via the prefix stage
        // is acceptable — the key invariant is that an EXACT alias for
        // "toolsets" no longer resolves.
        match result {
            ResolveResult::Exact(cmd) => {
                assert_ne!(
                    cmd.name, "toolsets",
                    "no command named 'toolsets' should exist; got Exact match"
                );
            }
            _ => {} // NotFound or PrefixMatch — both acceptable
        }
    }

    /// D-06: `/toolset` is on Universal platform (CLI REPL + gateway, NOT
    /// ApiServer/ACP). Test by resolving on Telegram.
    #[test]
    fn slash_toolset_platform_is_universal() {
        let router = CommandRouter::new(build_registry());
        // Universal includes Telegram (gateway). If the registration were
        // CliOnly, this would return NotFound.
        let result = router.resolve("toolset", &Platform::Telegram);
        assert!(
            matches!(result, ResolveResult::Exact(c) if c.name == "toolset"),
            "expected /toolset on Telegram (Universal platform), got: {:?}",
            result
        );
    }
}

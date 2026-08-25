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
        // Phase 32.3 Plan 03 (B3 / D-12 carve-out): args_hint extended with
        // interrupt|prune|status so `/help` operators see the full surface.
        CommandDef::new(
            "agents",
            "List, kill, interrupt, prune, or inspect active subagents",
            Session,
        )
        .args_hint("[list|kill <id>|interrupt <id>|prune|status <id>|logs <id>]")
        .platform(Universal),
        // Phase 32.3 Plan 03 (B3 / D-12 carve-out): the three new /agents
        // subcommands get sibling CommandDef entries so `/help` lists each
        // with a one-line description (mirrors the `"provider list"` /
        // `"provider show"` precedent above).
        CommandDef::new(
            "agents interrupt",
            "Soft-cancel a subagent (finalizes current iteration)",
            Session,
        )
        .args_hint("<id>")
        .platform(Universal),
        CommandDef::new(
            "agents prune",
            "Sweep registry entries idle longer than the stale threshold",
            Session,
        )
        .platform(Universal),
        CommandDef::new(
            "agents status",
            "Show diagnostic info for one active subagent",
            Session,
        )
        .args_hint("<id>")
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
        // Phase 36.17.3 (D-06 amended): /pause toggles queue drain pause.
        // Alias is /unpause (NOT /resume) — /resume already exists below as
        // "Resume a previous session". CliOnly because the queue-drain
        // pause/unpause flow is a TUI affordance; gateway adapters do not
        // expose user-facing pause toggles (see RESEARCH Pitfall 4).
        CommandDef::new("pause", "Pause or resume queue drain", Session)
            .aliases(&["unpause"])
            .platform(CliOnly),
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
        // The Telegram bot menu maps hyphens to underscores (Telegram only
        // allows [a-z0-9_] in command names), so the underscore form must
        // resolve too — mirrors the "reload-mcp"/"reload_mcp" pair below.
        .aliases(&["export_session"])
        .args_hint("[session_id]")
        .platform(Universal),
        // Phase 46.7 Plan 06 (D-18): queues a local file — copied into the
        // session attachment store immediately (D-20) — for the NEXT
        // submitted message. TUI/CLI-only (no gateway analog exists; the
        // gateway/Telegram surface is already covered by multimodal.rs).
        // The core dispatch table intentionally has no "attach" arm — it
        // falls through to `todo_stub` (never surfaced), because the real
        // handler needs App-side state (the pending-attachment queue,
        // StateStore, session_id) that CommandContext doesn't carry. See
        // `tui_rata::commands::dispatch_slash`'s post-router hook, which
        // mirrors the existing `"mouse"` App-side-handler pattern.
        CommandDef::new("attach", "Attach a file to your next message", Session)
            .args_hint("<path>")
            .platform(CliOnly),
        // Phase 36.6.4 Plan 05 (D-12/D-13, TUI-IMG-01): shows a clickable
        // image chip for a file on disk — mirrors `/attach`'s registration
        // exactly. The core dispatch table intentionally has no "image" arm
        // (falls through to `todo_stub`, never surfaced) because the real
        // handler needs App-side state (the image-chip collection, bounded
        // file-size check) that CommandContext doesn't carry. See
        // `tui_rata::commands::dispatch_slash`'s post-router hook, which
        // mirrors the existing `"attach"` App-side-handler pattern.
        CommandDef::new("image", "Show a clickable chip for an image file", Session)
            .args_hint("<path>")
            .platform(CliOnly),
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
        // Phase 41.1 (D-09): /skills registered Universal (was CliOnly) so the
        // catalog-listing command propagates to Web/Telegram via CommandRouter —
        // CliOnly made `CommandRouter::resolve` filter it out on those surfaces
        // regardless of `.with_skill_registry` wiring (Pitfall 2). Mirrors the
        // adjacent "kanban" Universal precedent. NOTE: individual skill
        // invocation (e.g. /gsd-config) is unaffected — it resolves via the
        // SKILL-13 NotFound fallback against the SkillRegistry, not this CommandDef.
        CommandDef::new("skills", "List installed skills", ToolsAndSkills).platform(Universal),
        CommandDef::new("cron", "Manage cron jobs", ToolsAndSkills)
            .args_hint("[subcommand]")
            .platform(CliOnly),
        // Phase 36.3.7 (D-35/D-36): /kanban slash command registered with Universal
        // platform so it propagates to TG/Discord/Slack/UI via CommandRouter. NOT
        // CliOnly (unlike cron) — D-36 requires mid-run bypass to work on gateway+UI.
        CommandDef::new("kanban", "Manage kanban board and tasks", ToolsAndSkills)
            .args_hint("[subcommand]")
            .platform(Universal),
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
        // Phase 36.2 Plan 10: `/usage` now wires through cmd_usage with flat-
        // flag filters; description + args_hint updated to reflect the new
        // subcommand surface (CommandRouter dispatch fans out to CLI / TUI /
        // gateway / web UI for free).
        CommandDef::new("usage", "Show token usage and costs", Info)
            .args_hint("[--today | --provider X | --since Nd | --model X]")
            .platform(Universal),
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
        CommandDef::new("profile", "Show active profile and IRONHERMES_HOME", Info)
            .platform(Universal),
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
        // NotFound or PrefixMatch — both acceptable; only Exact is a failure
        if let ResolveResult::Exact(cmd) = result {
            assert_ne!(
                cmd.name, "toolsets",
                "no command named 'toolsets' should exist; got Exact match"
            );
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

    /// Phase 46.7 Plan 06 (D-18): `/attach` registers as a CommandDef and
    /// `CommandRouter::new` does not panic (mirrors RESEARCH's
    /// "command_router_registers_attach" test).
    #[test]
    fn command_router_registers_attach() {
        let router = CommandRouter::new(build_registry());
        let result = router.resolve("attach", &Platform::Local);
        assert!(
            matches!(result, ResolveResult::Exact(c) if c.name == "attach"),
            "expected /attach Exact match, got: {:?}",
            result
        );
    }

    /// Phase 46.7 Plan 06 (D-18): `/attach` is CLI-only — no gateway analog
    /// (Telegram is already covered by multimodal.rs).
    #[test]
    fn slash_attach_not_available_on_gateway() {
        let router = CommandRouter::new(build_registry());
        let result = router.resolve("attach", &Platform::Telegram);
        assert!(
            !matches!(result, ResolveResult::Exact(c) if c.name == "attach"),
            "/attach must not resolve on the gateway platform, got: {:?}",
            result
        );
    }

    /// Phase 36.6.4 Plan 05 (TUI-IMG-01): `/image` registers as a CommandDef
    /// and resolves Exact on the CLI (Local) platform, and is CLI-only —
    /// does not resolve on the gateway (Telegram) platform. Mirrors
    /// `command_router_registers_attach` + `slash_attach_not_available_on_gateway`
    /// combined into one test per this plan's acceptance criterion name.
    #[test]
    fn image_slash_command_registers_and_is_cli_only() {
        let router = CommandRouter::new(build_registry());
        let local = router.resolve("image", &Platform::Local);
        assert!(
            matches!(local, ResolveResult::Exact(c) if c.name == "image"),
            "expected /image Exact match on Local, got: {:?}",
            local
        );
        let gateway = router.resolve("image", &Platform::Telegram);
        assert!(
            !matches!(gateway, ResolveResult::Exact(c) if c.name == "image"),
            "/image must not resolve on the gateway platform, got: {:?}",
            gateway
        );
    }
}

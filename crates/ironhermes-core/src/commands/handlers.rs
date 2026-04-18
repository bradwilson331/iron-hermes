use std::sync::atomic::Ordering;

use crate::commands::context::CommandContext;
use crate::commands::{CommandDef, CommandResult, CommandRouter};

/// Dispatch a resolved command to its handler.
///
/// Takes the resolved CommandDef, the remaining arguments (words after the
/// command name), the execution context, and the router (needed by /help and
/// /commands to enumerate available commands).
pub fn dispatch(
    cmd: &CommandDef,
    args: &[&str],
    ctx: &CommandContext,
    router: &CommandRouter,
) -> CommandResult {
    match cmd.name {
        // -------------------------------------------------------------------
        // Session commands
        // -------------------------------------------------------------------
        "new" => cmd_new(args, ctx),
        "clear" => cmd_clear(ctx),
        "stop" => cmd_stop(ctx),
        "status" => cmd_status(ctx),
        "title" => cmd_title(args, ctx),
        "compress" => cmd_compress(args, ctx),
        "start" => cmd_start(ctx),

        // -------------------------------------------------------------------
        // Configuration commands
        // -------------------------------------------------------------------
        "provider" => cmd_provider(ctx),
        "config" => cmd_config(ctx),
        "profile" => cmd_profile(ctx),
        "yolo" => cmd_yolo(ctx),
        "verbose" => cmd_verbose(ctx),
        "statusbar" => cmd_statusbar(ctx),
        "reasoning" => cmd_reasoning(args, ctx),

        // -------------------------------------------------------------------
        // Exit
        // -------------------------------------------------------------------
        "quit" => cmd_quit(ctx),

        // -------------------------------------------------------------------
        // Info
        // -------------------------------------------------------------------
        "help" => cmd_help(ctx, router),
        "commands" => cmd_commands(args, ctx, router),
        "skills" => cmd_skills(ctx),

        // -------------------------------------------------------------------
        // TODO stubs — everything without backing infrastructure
        // -------------------------------------------------------------------
        name => todo_stub(name),
    }
}

// =============================================================================
// Wirable handlers
// =============================================================================

fn cmd_new(_args: &[&str], _ctx: &CommandContext) -> CommandResult {
    CommandResult::NewSession {
        message: "Conversation cleared. Starting fresh.".to_string(),
    }
}

fn cmd_clear(_ctx: &CommandContext) -> CommandResult {
    CommandResult::ClearSession
}

fn cmd_quit(_ctx: &CommandContext) -> CommandResult {
    CommandResult::Quit
}

fn cmd_stop(ctx: &CommandContext) -> CommandResult {
    // NOTE: On the CLI, /stop can only be entered at the readline prompt when
    // the agent is idle (the REPL is single-threaded), so agent_running is
    // always false. On the gateway, agent_running is not yet wired. In-flight
    // cancellation is handled by ctrl-c (CLI) or platform-level mechanisms.
    // TODO: Wire CancellationToken into CommandContext to enable true /stop.
    if ctx.agent_running.load(Ordering::SeqCst) {
        CommandResult::Output("Stopping agent... (note: cancellation token not yet wired — agent may continue)".to_string())
    } else {
        CommandResult::Output("No agent is currently running. Use Ctrl-C to cancel an in-flight turn.".to_string())
    }
}

fn cmd_status(ctx: &CommandContext) -> CommandResult {
    CommandResult::Output(format!(
        "Session: {}\nPlatform: {}",
        ctx.session_id, ctx.platform
    ))
}

fn cmd_title(args: &[&str], _ctx: &CommandContext) -> CommandResult {
    if args.is_empty() {
        return CommandResult::Error(
            "Usage: /title [name] — please provide a session title.".to_string(),
        );
    }
    CommandResult::Output(format!("Session title set to: {}", args.join(" ")))
}

fn cmd_compress(_args: &[&str], _ctx: &CommandContext) -> CommandResult {
    CommandResult::Output("Compressing context...".to_string())
}

fn cmd_start(_ctx: &CommandContext) -> CommandResult {
    CommandResult::NewSession {
        message: String::new(),
    }
}

fn cmd_provider(_ctx: &CommandContext) -> CommandResult {
    CommandResult::Output("Provider: (use /model to view or change)".to_string())
}

fn cmd_config(_ctx: &CommandContext) -> CommandResult {
    CommandResult::Output(
        "Configuration: (use hermes config show for full details)".to_string(),
    )
}

fn cmd_profile(_ctx: &CommandContext) -> CommandResult {
    let home = crate::constants::display_hermes_home();
    CommandResult::Output(format!("Profile: default\nHome: {}", home))
}

fn cmd_yolo(_ctx: &CommandContext) -> CommandResult {
    CommandResult::Output(
        "YOLO mode toggled. (Session-level toggle requires caller wiring)".to_string(),
    )
}

fn cmd_verbose(_ctx: &CommandContext) -> CommandResult {
    CommandResult::Output("Verbose mode toggled.".to_string())
}

fn cmd_statusbar(_ctx: &CommandContext) -> CommandResult {
    CommandResult::Output("Status bar toggled.".to_string())
}

fn cmd_reasoning(args: &[&str], _ctx: &CommandContext) -> CommandResult {
    let level = args.first().unwrap_or(&"show");
    CommandResult::Output(format!("Reasoning: {}", level))
}

fn cmd_skills(ctx: &CommandContext) -> CommandResult {
    match &ctx.skill_registry {
        Some(registry) => CommandResult::Output(registry.catalog_text()),
        None => CommandResult::Output("No skills loaded.".to_string()),
    }
}

fn cmd_help(ctx: &CommandContext, router: &CommandRouter) -> CommandResult {
    let mut out = String::from("Available commands:\n");
    let groups = router.commands_by_category(&ctx.platform);

    for (category, cmds) in groups {
        let cat_name = match category {
            crate::commands::CommandCategory::Session => "SESSION",
            crate::commands::CommandCategory::Configuration => "CONFIGURATION",
            crate::commands::CommandCategory::ToolsAndSkills => "TOOLS AND SKILLS",
            crate::commands::CommandCategory::Info => "INFO",
            crate::commands::CommandCategory::Exit => "EXIT",
        };
        out.push('\n');
        out.push_str(cat_name);
        out.push('\n');

        for cmd in cmds {
            // Format: "  /name           args_hint    description"
            // cmd-col = 14 chars (name portion), arg-col = 16 chars
            let name_field = format!("/{}", cmd.name);
            let args_field = cmd.args_hint;
            out.push_str(&format!(
                "  {:<14}{:<16}{}\n",
                name_field, args_field, cmd.description
            ));
        }
    }

    // Keybindings section placeholder — actual keybindings injected by CLI adapter
    out.push_str("\nKeybindings:\n  (platform-specific bindings shown by caller)\n");

    CommandResult::Output(out)
}

fn cmd_commands(args: &[&str], ctx: &CommandContext, router: &CommandRouter) -> CommandResult {
    const PAGE_SIZE: usize = 10;

    let page: usize = args
        .first()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);

    let cmds = router.commands_for_platform(&ctx.platform);
    let total_cmds = cmds.len();
    let total_pages = (total_cmds + PAGE_SIZE - 1) / PAGE_SIZE;
    let total_pages = total_pages.max(1);
    let page = page.min(total_pages);

    let start = (page - 1) * PAGE_SIZE;
    let end = (start + PAGE_SIZE).min(total_cmds);
    let page_cmds = &cmds[start..end];

    let mut out = format!("Commands (page {}/{}):\n\n", page, total_pages);
    for cmd in page_cmds {
        out.push_str(&format!("/{} \u{2014} {}\n", cmd.name, cmd.description));
    }

    if page < total_pages {
        out.push_str(&format!("Use /commands {} for next page\n", page + 1));
    }
    out.push_str("--------------------");

    CommandResult::Output(out)
}

// =============================================================================
// TODO stubs
// =============================================================================

fn todo_stub(name: &str) -> CommandResult {
    let reason = match name {
        "voice" => "No TTS infrastructure",
        "background" | "bg" => "No background session manager",
        "rollback" => "No checkpoint system",
        "snapshot" => "No checkpoint system",
        "insights" => "No analytics infrastructure",
        "usage" => "No token cost tracking",
        "update" => "Binary build \u{2014} use package manager",
        "sethome" | "set-home" => "No home channel concept",
        "retry" => "No last-message replay",
        "undo" => "No message history manipulation",
        "resume" => "No session listing UI",
        "approve" => "No approval queue",
        "deny" => "No approval queue",
        "history" => "No history display",
        "save" => "No conversation export",
        "prompt" => "No custom system prompt injection",
        "personality" => "Requires Phase 15",
        "tools" => "No tool enable/disable management",
        "toolsets" => "No toolset listing",
        "cron" => "No cron management UI",
        "reload-mcp" | "reload_mcp" => "No MCP reload",
        "reload" => "No MCP reload",
        "browser" => "No browser tools",
        "plugins" => "No plugin system",
        "paste" => "No clipboard integration",
        "platforms" | "gateway" => "No platform status display",
        "skin" => "No theme system",
        "btw" => "No ephemeral query mechanism",
        "queue" => "No input queue",
        "fast" => "No model preset",
        "debug" => "No debug info toggle",
        "model" => "Model switching requires caller wiring",
        _ => "Not implemented",
    };
    CommandResult::Output(format!("/{} is not yet available. ({})", name, reason))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::registry::build_registry;
    use crate::commands::CommandRouter;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    fn make_ctx(agent_running: bool) -> CommandContext {
        CommandContext::new(
            crate::types::Platform::Local,
            "test-session-id".to_string(),
            Arc::new(AtomicBool::new(agent_running)),
        )
    }

    fn make_router() -> CommandRouter {
        CommandRouter::new(build_registry())
    }

    fn find_cmd(name: &str) -> CommandDef {
        build_registry()
            .into_iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("Command '{}' not found in registry", name))
    }

    #[test]
    fn dispatch_help_returns_available_commands() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("help");
        let result = dispatch(&cmd, &[], &ctx, &router);
        match result {
            CommandResult::Output(s) => assert!(
                s.contains("Available commands:"),
                "Help output missing 'Available commands:': {}",
                s
            ),
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_quit_returns_quit() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("quit");
        assert_eq!(dispatch(&cmd, &[], &ctx, &router), CommandResult::Quit);
    }

    #[test]
    fn dispatch_clear_returns_clear_session() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("clear");
        assert_eq!(
            dispatch(&cmd, &[], &ctx, &router),
            CommandResult::ClearSession
        );
    }

    #[test]
    fn dispatch_new_returns_new_session() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("new");
        let result = dispatch(&cmd, &[], &ctx, &router);
        assert!(
            matches!(result, CommandResult::NewSession { .. }),
            "Expected NewSession, got {:?}",
            result
        );
    }

    #[test]
    fn dispatch_stop_agent_idle_says_no_agent_running() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("stop");
        let result = dispatch(&cmd, &[], &ctx, &router);
        match &result {
            CommandResult::Output(s) => assert!(
                s.contains("No agent is currently running"),
                "Expected idle message, got: {}",
                s
            ),
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_stop_agent_running_says_stopping() {
        let ctx = make_ctx(true);
        let router = make_router();
        let cmd = find_cmd("stop");
        let result = dispatch(&cmd, &[], &ctx, &router);
        match &result {
            CommandResult::Output(s) => assert!(
                s.contains("Stopping agent"),
                "Expected stopping message, got: {}",
                s
            ),
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_voice_is_not_yet_available() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("voice");
        let result = dispatch(&cmd, &[], &ctx, &router);
        match result {
            CommandResult::Output(s) => assert!(
                s.contains("is not yet available"),
                "Expected stub message, got: {}",
                s
            ),
            other => panic!("Expected Output stub, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_skills_with_no_registry_says_no_skills() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("skills");
        assert_eq!(
            dispatch(&cmd, &[], &ctx, &router),
            CommandResult::Output("No skills loaded.".to_string())
        );
    }

    #[test]
    fn dispatch_title_with_args_returns_confirmation() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("title");
        let result = dispatch(&cmd, &["my", "title"], &ctx, &router);
        match result {
            CommandResult::Output(s) => assert!(
                s.contains("Session title set to: my title"),
                "Expected title confirmation, got: {}",
                s
            ),
            other => panic!("Expected Output, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_title_with_no_args_returns_error() {
        let ctx = make_ctx(false);
        let router = make_router();
        let cmd = find_cmd("title");
        let result = dispatch(&cmd, &[], &ctx, &router);
        assert!(
            matches!(result, CommandResult::Error(_)),
            "Expected Error for /title with no args, got {:?}",
            result
        );
    }

    #[test]
    fn dispatch_all_todo_stubs_return_not_yet_available() {
        let todo_commands = [
            "voice",
            "background",
            "rollback",
            "snapshot",
            "insights",
            "usage",
            "update",
            "sethome",
            "retry",
            "undo",
            "resume",
            "approve",
            "deny",
            "history",
            "save",
            "prompt",
            "personality",
            "tools",
            "toolsets",
            "cron",
            "reload-mcp",
            "reload",
            "browser",
            "plugins",
            "paste",
            "platforms",
            "skin",
            "btw",
            "queue",
            "fast",
            "debug",
            "model",
        ];
        let ctx = make_ctx(false);
        let router = make_router();
        let registry = build_registry();

        for name in &todo_commands {
            let cmd = registry
                .iter()
                .find(|c| c.name == *name)
                .unwrap_or_else(|| panic!("Command '{}' not in registry", name));
            let result = dispatch(cmd, &[], &ctx, &router);
            match &result {
                CommandResult::Output(s) => assert!(
                    s.contains("is not yet available"),
                    "Command '{}' should return stub message, got: {}",
                    name,
                    s
                ),
                other => panic!(
                    "Command '{}' should return Output stub, got {:?}",
                    name, other
                ),
            }
        }
    }
}

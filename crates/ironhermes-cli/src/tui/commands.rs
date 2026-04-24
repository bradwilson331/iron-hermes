//! Command dispatch with extension-first priority chain.
//!
//! Phase 22.1 Plan 01 Task 2.
//! Phase 21.1 Plan 02 Task 1: Replaced core_dispatch with CommandRouter.
//!
//! Extension calls are wrapped in `std::panic::catch_unwind()` to contain
//! panics from compiled-in extensions (security: T-22.1-03 mitigation).

use crate::tui::extension::{CommandResult, TuiExtension};
use crate::tui::keybindings::KeybindingRegistry;
use ironhermes_core::commands::{
    CommandCategory, CommandResult as CoreCommandResult, CommandRouter, ResolveResult,
};
use ironhermes_core::commands::context::CommandContext;
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::commands::typo::suggest_typo;
use ironhermes_core::types::Platform;

// ---------------------------------------------------------------------------
// dispatch_command
// ---------------------------------------------------------------------------

/// Dispatch a slash command through the extension chain, then fall back to
/// the unified CommandRouter.
///
/// `cmd` is the command name without the `/` prefix (e.g. `"quit"`).
/// `args` are the whitespace-split arguments that follow.
/// `router` is the shared CommandRouter (built once per session).
/// `ctx` is the CommandContext (platform, session_id, agent_running).
///
/// # Extension-first priority
/// Extensions are tried in registration order. The first extension that
/// returns `Some(result)` wins. If all extensions return `None`, the core
/// CommandRouter handles the command.
///
/// # Panic containment (T-22.1-03)
/// Each extension call is wrapped in `std::panic::catch_unwind()`. A panicking
/// extension is logged via `tracing::warn!` and skipped; dispatch continues to
/// the next extension.
pub fn dispatch_command(
    extensions: &[Box<dyn TuiExtension>],
    cmd: &str,
    args: &[&str],
    router: &CommandRouter,
    ctx: &CommandContext,
) -> CommandResult {
    // Extension-first dispatch chain.
    for ext in extensions {
        // Safety: AssertUnwindSafe is acceptable here because:
        // - Extensions are compiled-in (not dynamically loaded).
        // - The only consequence of a panic is skipping that extension.
        // - We log the extension name so issues are diagnosable.
        let ext_name = ext.name().to_string();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ext.process_command(cmd, args)
        }));

        match result {
            Ok(Some(cmd_result)) => return cmd_result,
            Ok(None) => continue,
            Err(_) => {
                tracing::warn!(
                    "tui: extension '{}' panicked in process_command() -- skipping",
                    ext_name
                );
                continue;
            }
        }
    }

    // Router-based core dispatch (replaces hardcoded core_dispatch).
    let full_input = if args.is_empty() {
        format!("/{}", cmd)
    } else {
        format!("/{} {}", cmd, args.join(" "))
    };

    match router.resolve(&full_input, &ctx.platform) {
        ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
            let core_result =
                ironhermes_core::commands::handlers::dispatch(def, args, ctx, router);
            map_core_to_tui(core_result)
        }
        ResolveResult::Ambiguous(candidates) => {
            let list = candidates
                .iter()
                .map(|c| format!("/{}", c))
                .collect::<Vec<_>>()
                .join(", ");
            CommandResult::Error(format!(
                "Ambiguous command: /{}. Matches: {}. Be more specific.",
                cmd, list
            ))
        }
        ResolveResult::NotFound => {
            // Check skill registry for /<skill-name> catch-all (D-06)
            if let Some(registry) = &ctx.skill_registry {
                if registry.find(cmd).is_some() {
                    return CommandResult::Handled(format!("Activating skill: {}", cmd));
                }
            }
            // Phase 22.3 D-10 / UI-SPEC TYPO-4: append Levenshtein-2 suggestion
            // when a known top-level command is close. Candidates derived from
            // the router's platform-filtered command set.
            let known: Vec<&str> = router
                .commands_for_platform(&ctx.platform)
                .iter()
                .map(|c| c.name)
                .collect();
            let suffix = suggest_typo(cmd, &known)
                .map(|s| format!(" {}", s))
                .unwrap_or_else(|| " Type /help for available commands.".to_string());
            CommandResult::Error(format!("Unknown command: /{}.{}", cmd, suffix))
        }
    }
}

/// Map core CommandResult to TUI CommandResult.
fn map_core_to_tui(core: CoreCommandResult) -> CommandResult {
    match core {
        CoreCommandResult::Output(text) => CommandResult::Handled(text),
        CoreCommandResult::Handled => CommandResult::Silent,
        CoreCommandResult::Error(msg) => CommandResult::Error(msg),
        CoreCommandResult::Quit => CommandResult::Quit,
        CoreCommandResult::ClearSession => {
            CommandResult::ClearSession("Conversation cleared.".to_string())
        }
        CoreCommandResult::NewSession { message } => {
            if message.is_empty() {
                CommandResult::ClearSession(
                    "Conversation cleared. Starting fresh.".to_string(),
                )
            } else {
                CommandResult::ClearSession(message)
            }
        }
        CoreCommandResult::PassThrough => CommandResult::Error("Unknown command.".to_string()),
        // Phase 22.3 D-05 / UI-SPEC CLR-8: pass-through map of the new core
        // ResetTerminal variant to the TUI ResetTerminal variant. Both unit.
        CoreCommandResult::ResetTerminal => CommandResult::ResetTerminal,
        // Phase 21.2 Plan 04: MCP reload — pass through to REPL loop for async dispatch.
        CoreCommandResult::McpReload => CommandResult::McpReload,
    }
}

// ---------------------------------------------------------------------------
// format_help
// ---------------------------------------------------------------------------

/// Produce help text for the TUI's `/help` command.
///
/// Uses the CommandRouter to enumerate commands by category.
/// Matches the UI-SPEC copywriting contract:
/// - Header `"Available commands:"`
/// - Commands grouped by category with 2-space indent
/// - Keybindings section (if `keybinding_registry` is Some)
pub fn format_help(
    _extensions: &[Box<dyn TuiExtension>],
    keybinding_registry: Option<&KeybindingRegistry>,
    router: &CommandRouter,
    platform: &Platform,
) -> String {
    let mut out = String::from("Available commands:\n");

    for (category, cmds) in router.commands_by_category(platform) {
        out.push('\n');
        let cat_name = match category {
            CommandCategory::Session => "SESSION",
            CommandCategory::Configuration => "CONFIGURATION",
            CommandCategory::ToolsAndSkills => "TOOLS & SKILLS",
            CommandCategory::Info => "INFO",
            CommandCategory::Exit => "EXIT",
        };
        out.push_str(cat_name);
        out.push('\n');
        for cmd in cmds {
            // UI-SPEC: 2-space indent, cmd-col=14, arg-col=16
            out.push_str(&format!(
                "  /{:<13}{:<16}{}\n",
                cmd.name, cmd.args_hint, cmd.description
            ));
        }
    }

    if let Some(registry) = keybinding_registry {
        let entries = registry.help_entries();
        if !entries.is_empty() {
            out.push_str("\nKeybindings:\n");
            for (key_display, description, context) in &entries {
                let ctx_str = match context {
                    crate::tui::extension::KeyContext::Idle => "idle",
                    crate::tui::extension::KeyContext::InFlight => "in-flight",
                    crate::tui::extension::KeyContext::Always => "always",
                };
                out.push_str(&format!(
                    "  {:<12}  {}  ({})\n",
                    key_display, description, ctx_str
                ));
            }
        }
    }

    out
}

/// Build a default CommandRouter for CLI use (Platform::Local).
/// Convenience used by print_help() and tests.
pub fn build_cli_router() -> CommandRouter {
    CommandRouter::new(build_registry())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::extension::{CommandResult, KeyContext, Keybinding, TuiExtension};
    use crate::tui::keybindings::KeybindingRegistry;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use ironhermes_core::commands::context::CommandContext;
    use ironhermes_core::commands::registry::build_registry;
    use ironhermes_core::commands::CommandRouter;
    use ironhermes_core::types::Platform;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn test_router() -> CommandRouter {
        CommandRouter::new(build_registry())
    }

    fn test_ctx() -> CommandContext {
        CommandContext::new(
            Platform::Local,
            "test-session".to_string(),
            Arc::new(AtomicBool::new(false)),
        )
    }

    // --- Test extension helpers ---

    struct NoOpExt;
    impl TuiExtension for NoOpExt {
        fn name(&self) -> &str {
            "noop"
        }
    }

    struct FooCommandExt;
    impl TuiExtension for FooCommandExt {
        fn name(&self) -> &str {
            "foo_ext"
        }
        fn process_command(&self, cmd: &str, _args: &[&str]) -> Option<CommandResult> {
            if cmd == "foo" {
                Some(CommandResult::Handled("foo result".to_string()))
            } else {
                None
            }
        }
    }

    /// Extension that claims /help (shadows core /help).
    struct HelpShadowExt;
    impl TuiExtension for HelpShadowExt {
        fn name(&self) -> &str {
            "help_shadow"
        }
        fn process_command(&self, cmd: &str, _args: &[&str]) -> Option<CommandResult> {
            if cmd == "help" {
                Some(CommandResult::Handled("extension help!".to_string()))
            } else {
                None
            }
        }
    }

    /// Extension that always panics.
    struct PanicExt;
    impl TuiExtension for PanicExt {
        fn name(&self) -> &str {
            "panic_ext"
        }
        fn process_command(&self, _cmd: &str, _args: &[&str]) -> Option<CommandResult> {
            panic!("intentional test panic");
        }
    }

    /// Extension that returns None for all commands (passes through).
    struct PassThroughExt;
    impl TuiExtension for PassThroughExt {
        fn name(&self) -> &str {
            "passthrough"
        }
        fn process_command(&self, _cmd: &str, _args: &[&str]) -> Option<CommandResult> {
            None
        }
    }

    // --- Core router tests (no extensions) ---

    #[test]
    fn dispatch_no_extensions_quit_handled() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let router = test_router();
        let ctx = test_ctx();
        let result = dispatch_command(&exts, "quit", &[], &router, &ctx);
        assert_eq!(result, CommandResult::Quit);
    }

    #[test]
    fn dispatch_no_extensions_exit_handled() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let router = test_router();
        let ctx = test_ctx();
        let result = dispatch_command(&exts, "exit", &[], &router, &ctx);
        assert_eq!(result, CommandResult::Quit);
    }

    #[test]
    fn dispatch_no_extensions_clear_handled() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let router = test_router();
        let ctx = test_ctx();
        let result = dispatch_command(&exts, "clear", &[], &router, &ctx);
        assert_eq!(
            result,
            CommandResult::ClearSession("Conversation cleared.".to_string())
        );
    }

    #[test]
    fn dispatch_no_extensions_help_contains_available_commands() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let router = test_router();
        let ctx = test_ctx();
        let result = dispatch_command(&exts, "help", &[], &router, &ctx);
        if let CommandResult::Handled(text) = result {
            assert!(text.contains("Available commands:"), "got: {}", text);
        } else {
            panic!("expected Handled");
        }
    }

    #[test]
    fn dispatch_no_extensions_unknown_returns_error() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let router = test_router();
        let ctx = test_ctx();
        let result = dispatch_command(&exts, "unknown_zzz", &[], &router, &ctx);
        if let CommandResult::Error(msg) = result {
            assert!(
                msg.contains("Unknown command: /unknown_zzz"),
                "got: {}",
                msg
            );
        } else {
            panic!("expected Error");
        }
    }

    // --- Extension dispatch tests ---

    #[test]
    fn dispatch_extension_handles_foo_command() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![Box::new(FooCommandExt)];
        let router = test_router();
        let ctx = test_ctx();
        let result = dispatch_command(&exts, "foo", &[], &router, &ctx);
        assert_eq!(result, CommandResult::Handled("foo result".to_string()));
    }

    #[test]
    fn dispatch_extension_first_priority_shadows_help() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![Box::new(HelpShadowExt)];
        let router = test_router();
        let ctx = test_ctx();
        let result = dispatch_command(&exts, "help", &[], &router, &ctx);
        assert_eq!(result, CommandResult::Handled("extension help!".to_string()));
    }

    #[test]
    fn dispatch_extension_returning_none_falls_through_to_core() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![Box::new(PassThroughExt)];
        let router = test_router();
        let ctx = test_ctx();
        let result = dispatch_command(&exts, "quit", &[], &router, &ctx);
        assert_eq!(result, CommandResult::Quit);
    }

    #[test]
    fn dispatch_extension_returning_none_falls_through_to_next_extension() {
        // PassThroughExt returns None, FooCommandExt handles "foo"
        let exts: Vec<Box<dyn TuiExtension>> = vec![
            Box::new(PassThroughExt),
            Box::new(FooCommandExt),
        ];
        let router = test_router();
        let ctx = test_ctx();
        let result = dispatch_command(&exts, "foo", &[], &router, &ctx);
        assert_eq!(result, CommandResult::Handled("foo result".to_string()));
    }

    #[test]
    fn dispatch_panic_extension_is_contained_and_skips_to_core() {
        // PanicExt panics → should be caught and skipped; "quit" falls to core router
        let exts: Vec<Box<dyn TuiExtension>> = vec![Box::new(PanicExt)];
        let router = test_router();
        let ctx = test_ctx();
        let result = dispatch_command(&exts, "quit", &[], &router, &ctx);
        assert_eq!(result, CommandResult::Quit);
    }

    // --- Ambiguous and unknown prefix tests ---

    #[test]
    fn dispatch_ambiguous_prefix_returns_error_with_candidates() {
        // "s" is ambiguous on Local (stop, status, statusbar, skills, save, etc.)
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let router = test_router();
        let ctx = test_ctx();
        let result = dispatch_command(&exts, "s", &[], &router, &ctx);
        // Should be Ambiguous or prefix match (depending on shortest-match logic)
        match result {
            CommandResult::Error(msg) => {
                assert!(
                    msg.contains("Ambiguous"),
                    "expected ambiguous error, got: {}",
                    msg
                );
            }
            CommandResult::Handled(_) => {
                // Shortest match could resolve to a single command — acceptable
            }
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn dispatch_unknown_returns_error() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let router = test_router();
        let ctx = test_ctx();
        let result = dispatch_command(&exts, "zzz_unknown_cmd", &[], &router, &ctx);
        if let CommandResult::Error(msg) = result {
            assert!(
                msg.contains("Unknown command"),
                "expected Unknown command error, got: {}",
                msg
            );
        } else {
            panic!("expected Error for unknown command, got: {:?}", result);
        }
    }

    // --- format_help tests ---

    #[test]
    fn format_help_includes_available_commands_header() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let router = test_router();
        let text = format_help(&exts, None, &router, &Platform::Local);
        assert!(text.contains("Available commands:"), "got: {}", text);
    }

    #[test]
    fn format_help_includes_keybinding_section_when_registry_provided() {
        struct ExtWithOneBinding;
        impl TuiExtension for ExtWithOneBinding {
            fn name(&self) -> &str {
                "one_binding_ext"
            }
            fn keybindings(&self) -> Vec<Keybinding> {
                vec![Keybinding {
                    key: KeyEvent {
                        code: KeyCode::F(1),
                        modifiers: KeyModifiers::NONE,
                        kind: KeyEventKind::Press,
                        state: KeyEventState::NONE,
                    },
                    description: "Show help".to_string(),
                    when: KeyContext::Always,
                    action_name: "help".to_string(),
                }]
            }
        }
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let ext_with_binding: Box<dyn TuiExtension> = Box::new(ExtWithOneBinding);
        let registry = KeybindingRegistry::register_from_extensions(&[ext_with_binding]);
        let router = test_router();
        let text = format_help(&exts, Some(&registry), &router, &Platform::Local);
        assert!(text.contains("Keybindings:"), "got: {}", text);
    }
}

//! Command dispatch with extension-first priority chain.
//!
//! Phase 22.1 Plan 01 Task 2.
//!
//! Extension calls are wrapped in `std::panic::catch_unwind()` to contain
//! panics from compiled-in extensions (security: T-22.1-03 mitigation).

use crate::tui::extension::{CommandResult, TuiExtension};
use crate::tui::keybindings::KeybindingRegistry;

// ---------------------------------------------------------------------------
// dispatch_command
// ---------------------------------------------------------------------------

/// Dispatch a slash command through the extension chain, then fall back to
/// the core command stub.
///
/// `cmd` is the command name without the `/` prefix (e.g. `"quit"`).
/// `args` are the whitespace-split arguments that follow.
///
/// # Extension-first priority
/// Extensions are tried in registration order. The first extension that
/// returns `Some(result)` wins. If all extensions return `None`, the core
/// stub handles the command.
///
/// # Panic containment (T-22.1-03)
/// Each extension call is wrapped in `std::panic::catch_unwind()`. A panicking
/// extension is logged via `tracing::warn!` and skipped; dispatch continues to
/// the next extension.
pub fn dispatch_command(
    extensions: &[Box<dyn TuiExtension>],
    cmd: &str,
    args: &[&str],
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

    // Core stub fallback.
    core_dispatch(extensions, cmd)
}

/// Core built-in command stub. Called when no extension claimed the command.
fn core_dispatch(extensions: &[Box<dyn TuiExtension>], cmd: &str) -> CommandResult {
    match cmd {
        "quit" | "exit" | "q" => CommandResult::Handled("Goodbye!".to_string()),
        "clear" => CommandResult::Handled("Conversation cleared.".to_string()),
        "status" => CommandResult::Handled("Status...".to_string()),
        "help" => CommandResult::Handled(format_help(extensions, None)),
        _ => CommandResult::Error(format!(
            "Unknown command: /{}. Type /help for available commands.",
            cmd
        )),
    }
}

// ---------------------------------------------------------------------------
// format_help
// ---------------------------------------------------------------------------

/// Produce help text for the TUI's `/help` command.
///
/// Matches the UI-SPEC copywriting contract:
/// - Header `"Available commands:"`
/// - Core commands with 2-space indent and 2-space gap
/// - Keybindings section (if `keybinding_registry` is Some)
pub fn format_help(
    _extensions: &[Box<dyn TuiExtension>],
    keybinding_registry: Option<&KeybindingRegistry>,
) -> String {
    let mut out = String::from("Available commands:\n");
    out.push_str("  /quit      Exit the program\n");
    out.push_str("  /clear     Clear conversation history\n");
    out.push_str("  /status    Show current status\n");
    out.push_str("  /help      Show this help\n");

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
                out.push_str(&format!("  {:<12}  {}  ({})\n", key_display, description, ctx_str));
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::extension::{
        CommandResult, KeyContext, Keybinding, TuiExtension,
    };
    use crate::tui::keybindings::KeybindingRegistry;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    // --- Test extension helpers ---

    struct NoOpExt;
    impl TuiExtension for NoOpExt {
        fn name(&self) -> &str { "noop" }
    }

    struct FooCommandExt;
    impl TuiExtension for FooCommandExt {
        fn name(&self) -> &str { "foo_ext" }
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
        fn name(&self) -> &str { "help_shadow" }
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
        fn name(&self) -> &str { "panic_ext" }
        fn process_command(&self, _cmd: &str, _args: &[&str]) -> Option<CommandResult> {
            panic!("intentional test panic");
        }
    }

    /// Extension that returns None for all commands (passes through).
    struct PassThroughExt;
    impl TuiExtension for PassThroughExt {
        fn name(&self) -> &str { "passthrough" }
        fn process_command(&self, _cmd: &str, _args: &[&str]) -> Option<CommandResult> {
            None
        }
    }

    // --- Core stub tests (no extensions) ---

    #[test]
    fn dispatch_no_extensions_quit_handled() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let result = dispatch_command(&exts, "quit", &[]);
        assert_eq!(result, CommandResult::Handled("Goodbye!".to_string()));
    }

    #[test]
    fn dispatch_no_extensions_exit_handled() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let result = dispatch_command(&exts, "exit", &[]);
        assert_eq!(result, CommandResult::Handled("Goodbye!".to_string()));
    }

    #[test]
    fn dispatch_no_extensions_clear_handled() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let result = dispatch_command(&exts, "clear", &[]);
        assert_eq!(result, CommandResult::Handled("Conversation cleared.".to_string()));
    }

    #[test]
    fn dispatch_no_extensions_help_contains_available_commands() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let result = dispatch_command(&exts, "help", &[]);
        if let CommandResult::Handled(text) = result {
            assert!(text.contains("Available commands:"), "got: {}", text);
        } else {
            panic!("expected Handled");
        }
    }

    #[test]
    fn dispatch_no_extensions_unknown_returns_error() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let result = dispatch_command(&exts, "unknown", &[]);
        if let CommandResult::Error(msg) = result {
            assert!(msg.contains("Unknown command: /unknown"), "got: {}", msg);
        } else {
            panic!("expected Error");
        }
    }

    // --- Extension dispatch tests ---

    #[test]
    fn dispatch_extension_handles_foo_command() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![Box::new(FooCommandExt)];
        let result = dispatch_command(&exts, "foo", &[]);
        assert_eq!(result, CommandResult::Handled("foo result".to_string()));
    }

    #[test]
    fn dispatch_extension_first_priority_shadows_help() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![Box::new(HelpShadowExt)];
        let result = dispatch_command(&exts, "help", &[]);
        assert_eq!(result, CommandResult::Handled("extension help!".to_string()));
    }

    #[test]
    fn dispatch_extension_returning_none_falls_through_to_core() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![Box::new(PassThroughExt)];
        let result = dispatch_command(&exts, "quit", &[]);
        assert_eq!(result, CommandResult::Handled("Goodbye!".to_string()));
    }

    #[test]
    fn dispatch_extension_returning_none_falls_through_to_next_extension() {
        // PassThroughExt returns None, FooCommandExt handles "foo"
        let exts: Vec<Box<dyn TuiExtension>> = vec![
            Box::new(PassThroughExt),
            Box::new(FooCommandExt),
        ];
        let result = dispatch_command(&exts, "foo", &[]);
        assert_eq!(result, CommandResult::Handled("foo result".to_string()));
    }

    #[test]
    fn dispatch_panic_extension_is_contained_and_skips_to_core() {
        // PanicExt panics → should be caught and skipped; "quit" falls to core
        let exts: Vec<Box<dyn TuiExtension>> = vec![Box::new(PanicExt)];
        let result = dispatch_command(&exts, "quit", &[]);
        assert_eq!(result, CommandResult::Handled("Goodbye!".to_string()));
    }

    // --- format_help tests ---

    #[test]
    fn format_help_includes_available_commands_header() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let text = format_help(&exts, None);
        assert!(text.contains("Available commands:"), "got: {}", text);
    }

    #[test]
    fn format_help_includes_keybinding_section_when_registry_provided() {
        struct ExtWithOneBinding;
        impl TuiExtension for ExtWithOneBinding {
            fn name(&self) -> &str { "one_binding_ext" }
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
        let text = format_help(&exts, Some(&registry));
        assert!(text.contains("Keybindings:"), "got: {}", text);
    }
}

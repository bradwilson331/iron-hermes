//! TUI extension system type contracts — Phase 22.1 Plan 01.
//!
//! Defines the pure-function contracts for the TUI extension system:
//! TuiExtension trait, Widget/LayoutSlot/TuiEvent types, StyleOverrides,
//! Keybinding, and KeyContext.
//!
//! Per D-01 through D-06 (22.1-CONTEXT.md): all types are pure (no I/O),
//! Send + Sync (for use with tokio tasks and Arc), and carry no terminal state.

use std::collections::HashMap;

/// Maximum widget height in terminal rows (security: T-22.1-01 mitigation).
pub const MAX_WIDGET_HEIGHT: u16 = 10;

// ---------------------------------------------------------------------------
// LayoutSlot — where widgets are placed in the TUI layout (D-03)
// ---------------------------------------------------------------------------

/// Defines the named region where an extension widget is rendered.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LayoutSlot {
    /// Row immediately above the status bar.
    AboveStatus,
    /// Row immediately below the knight-rider scanner.
    BelowScanner,
    /// Inline region to the right of the status pills.
    StatusRight,
}

// ---------------------------------------------------------------------------
// Widget — a pre-rendered ANSI string block (D-04)
// ---------------------------------------------------------------------------

/// A pre-rendered ANSI string block supplied by a TUI extension.
#[derive(Debug, Clone)]
pub struct Widget {
    /// Unique identifier (used as map key, prefixed with extension name).
    pub id: String,
    /// Pre-rendered ANSI string content (may contain escape codes).
    pub content: String,
    /// Height in terminal rows. Minimum 1, capped at [`MAX_WIDGET_HEIGHT`].
    pub height: u16,
}

impl Widget {
    /// Construct a Widget, capping `height` at [`MAX_WIDGET_HEIGHT`].
    pub fn new(id: impl Into<String>, content: impl Into<String>, height: u16) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            height: height.min(MAX_WIDGET_HEIGHT).max(1),
        }
    }

    /// Truncate each line of content to `max_width` characters (security:
    /// T-22.1-01 — prevents oversized content from corrupting the layout).
    /// This operates on char boundaries, not byte boundaries.
    pub fn truncate_content(&mut self, max_width: u16) {
        let max = max_width as usize;
        self.content = self
            .content
            .lines()
            .map(|line| {
                let chars: Vec<char> = line.chars().collect();
                if chars.len() > max {
                    chars[..max].iter().collect::<String>()
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
}

// ---------------------------------------------------------------------------
// KeyContext — when a keybinding is active (D-05)
// ---------------------------------------------------------------------------

/// Describes when a keybinding is active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyContext {
    /// Active only when the agent is idle (no in-flight request).
    Idle,
    /// Active only when a request is in-flight.
    InFlight,
    /// Active in both Idle and InFlight states.
    Always,
}

// ---------------------------------------------------------------------------
// Keybinding — a single keyboard shortcut registered by an extension (D-05)
// ---------------------------------------------------------------------------

/// A single keyboard shortcut registered by a TUI extension.
#[derive(Debug, Clone)]
pub struct Keybinding {
    /// The key event that triggers this binding.
    pub key: crossterm::event::KeyEvent,
    /// Human-readable description (shown in /help output).
    pub description: String,
    /// When this binding is active.
    pub when: KeyContext,
    /// Name of the action dispatched when this binding fires.
    pub action_name: String,
}

// ---------------------------------------------------------------------------
// CommandResult — outcome of dispatching a slash command (D-06)
// ---------------------------------------------------------------------------

/// The result of dispatching a `/command` through the extension chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandResult {
    /// Command handled; `String` is displayed to the user.
    Handled(String),
    /// Command handled silently (no output to user).
    Silent,
    /// Command failed; `String` is an error message displayed to the user.
    Error(String),
    /// Exit the application (maps from core `Quit`).
    Quit,
    /// Clear session history (maps from core `ClearSession` / `NewSession`).
    ClearSession(String),
    /// Request the REPL loop to perform an async MCP reload (Phase 21.2 Plan 04).
    /// Maps from `CoreCommandResult::McpReload`. The REPL loop calls McpReloader
    /// and formats the UI-SPEC status string including partial failure display.
    McpReload,
}

// ---------------------------------------------------------------------------
// StyleOverrides — extension-provided color customisations
// ---------------------------------------------------------------------------

/// A map of style-slot names to ANSI color name strings.
///
/// Keys are slot names (e.g. "scanner.lit", "scanner.trail", "scanner.bg",
/// "status.separator", "status.hint"). Values are color names from the
/// `colored` crate palette (e.g. "cyan", "bright red").
pub type StyleOverrides = HashMap<String, String>;

// ---------------------------------------------------------------------------
// TuiEvent — events extensions can push to the TUI event bus (D-02)
// ---------------------------------------------------------------------------

/// An event that an extension can publish to the TUI render loop.
#[derive(Debug, Clone)]
pub enum TuiEvent {
    /// Insert or replace a widget identified by `id`.
    UpdateWidget {
        id: String,
        content: String,
        height: u16,
    },
    /// Remove the widget with the given `id`.
    RemoveWidget { id: String },
    /// Flash a transient hint message in the status bar for `duration_ticks` frames.
    FlashHint { message: String, duration_ticks: u16 },
}

// ---------------------------------------------------------------------------
// TuiExtension trait — the extension contract (D-01)
// ---------------------------------------------------------------------------

/// The contract that a compiled-in TUI extension must satisfy.
///
/// All methods have default no-op implementations so extensions only override
/// what they need. The `name()` method has no default — every extension must
/// provide a unique name used for debug logging and widget ID prefixing (Pitfall 4).
///
/// # Thread Safety
/// `TuiExtension: Send + Sync` is required so extensions can be stored in
/// `Arc<dyn TuiExtension>` and shared across tokio tasks.
pub trait TuiExtension: Send + Sync {
    /// Unique name for this extension. Used for debug logging and as prefix
    /// for widget IDs to prevent collisions across extensions.
    fn name(&self) -> &str;

    /// Return the list of widgets this extension wants rendered, keyed by
    /// their target layout slot. Default: empty (no widgets).
    fn widgets(&self) -> Vec<(LayoutSlot, Widget)> {
        Vec::new()
    }

    /// Return the list of keybindings this extension registers.
    /// Default: empty (no keybindings).
    fn keybindings(&self) -> Vec<Keybinding> {
        Vec::new()
    }

    /// Attempt to handle a slash command.
    ///
    /// `cmd` is the command name without the `/` prefix (e.g. "help").
    /// `args` are the whitespace-split arguments.
    ///
    /// Return `Some(result)` to claim the command; `None` to pass through to
    /// the next extension or the core stub.
    ///
    /// Default: always returns `None` (passes through).
    fn process_command(&self, cmd: &str, args: &[&str]) -> Option<CommandResult> {
        let _ = (cmd, args);
        None
    }

    /// Return style overrides for named TUI slots.
    /// Default: empty map (no overrides).
    fn style_overrides(&self) -> StyleOverrides {
        StyleOverrides::default()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    /// Minimal concrete extension used in trait-object dispatch tests.
    struct TestExtension {
        name: &'static str,
    }

    impl TuiExtension for TestExtension {
        fn name(&self) -> &str {
            self.name
        }
    }

    fn make_key_event(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    // --- TuiExtension default implementations ---

    #[test]
    fn default_widgets_returns_empty() {
        let ext = TestExtension { name: "test" };
        assert!(ext.widgets().is_empty());
    }

    #[test]
    fn default_keybindings_returns_empty() {
        let ext = TestExtension { name: "test" };
        assert!(ext.keybindings().is_empty());
    }

    #[test]
    fn default_process_command_returns_none() {
        let ext = TestExtension { name: "test" };
        assert!(ext.process_command("quit", &[]).is_none());
        assert!(ext.process_command("unknown", &["arg1"]).is_none());
    }

    #[test]
    fn default_style_overrides_returns_empty_map() {
        let ext = TestExtension { name: "test" };
        assert!(ext.style_overrides().is_empty());
    }

    // --- Widget ---

    #[test]
    fn widget_new_constructs_with_given_fields() {
        let w = Widget::new("my-widget", "hello world", 3);
        assert_eq!(w.id, "my-widget");
        assert_eq!(w.content, "hello world");
        assert_eq!(w.height, 3);
    }

    #[test]
    fn widget_height_capped_at_max() {
        let w = Widget::new("id", "content", 100);
        assert_eq!(w.height, MAX_WIDGET_HEIGHT);
    }

    #[test]
    fn widget_height_minimum_one() {
        let w = Widget::new("id", "content", 0);
        assert_eq!(w.height, 1);
    }

    // --- LayoutSlot ---

    #[test]
    fn layout_slot_has_expected_variants() {
        let _above = LayoutSlot::AboveStatus;
        let _below = LayoutSlot::BelowScanner;
        let _right = LayoutSlot::StatusRight;
        // Equality and hashing
        assert_eq!(LayoutSlot::AboveStatus, LayoutSlot::AboveStatus);
        assert_ne!(LayoutSlot::AboveStatus, LayoutSlot::BelowScanner);
        let mut map = std::collections::HashMap::new();
        map.insert(LayoutSlot::StatusRight, 1u32);
        assert_eq!(map[&LayoutSlot::StatusRight], 1);
    }

    // --- TuiEvent ---

    #[test]
    fn tui_event_update_widget_carries_fields() {
        let ev = TuiEvent::UpdateWidget {
            id: "w1".to_string(),
            content: "hello".to_string(),
            height: 2,
        };
        if let TuiEvent::UpdateWidget { id, content, height } = ev {
            assert_eq!(id, "w1");
            assert_eq!(content, "hello");
            assert_eq!(height, 2);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn tui_event_remove_widget_carries_id() {
        let ev = TuiEvent::RemoveWidget { id: "w2".to_string() };
        if let TuiEvent::RemoveWidget { id } = ev {
            assert_eq!(id, "w2");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn tui_event_flash_hint_carries_fields() {
        let ev = TuiEvent::FlashHint {
            message: "done!".to_string(),
            duration_ticks: 5,
        };
        if let TuiEvent::FlashHint { message, duration_ticks } = ev {
            assert_eq!(message, "done!");
            assert_eq!(duration_ticks, 5);
        } else {
            panic!("wrong variant");
        }
    }

    // --- StyleOverrides ---

    #[test]
    fn style_overrides_default_is_empty() {
        let s: StyleOverrides = StyleOverrides::default();
        assert!(s.is_empty());
    }

    #[test]
    fn style_overrides_insert_and_retrieve() {
        let mut s: StyleOverrides = StyleOverrides::default();
        s.insert("scanner.lit".to_string(), "cyan".to_string());
        s.insert("status.hint".to_string(), "bright red".to_string());
        assert_eq!(s["scanner.lit"], "cyan");
        assert_eq!(s["status.hint"], "bright red");
    }

    // --- Keybinding / KeyContext ---

    #[test]
    fn keybinding_struct_holds_all_fields() {
        let kb = Keybinding {
            key: make_key_event(KeyCode::Char('t')),
            description: "Toggle panel".to_string(),
            when: KeyContext::Idle,
            action_name: "toggle_panel".to_string(),
        };
        assert_eq!(kb.description, "Toggle panel");
        assert_eq!(kb.when, KeyContext::Idle);
        assert_eq!(kb.action_name, "toggle_panel");
        assert_eq!(kb.key.code, KeyCode::Char('t'));
    }

    #[test]
    fn key_context_has_idle_inflight_always_variants() {
        assert_ne!(KeyContext::Idle, KeyContext::InFlight);
        assert_ne!(KeyContext::InFlight, KeyContext::Always);
        assert_eq!(KeyContext::Always, KeyContext::Always);
    }

    // --- Trait-object dispatch ---

    #[test]
    fn dyn_tui_extension_box_dispatch_works() {
        let ext: Box<dyn TuiExtension> = Box::new(TestExtension { name: "boxed" });
        assert_eq!(ext.name(), "boxed");
        assert!(ext.widgets().is_empty());
        assert!(ext.keybindings().is_empty());
        assert!(ext.process_command("x", &[]).is_none());
        assert!(ext.style_overrides().is_empty());
    }

    // --- Widget content truncation (security T-22.1-01) ---

    #[test]
    fn widget_truncate_content_at_max_width() {
        let mut w = Widget::new("id", "abcdefghij\n12345", 1);
        w.truncate_content(5);
        let lines: Vec<&str> = w.content.lines().collect();
        assert_eq!(lines[0], "abcde");
        assert_eq!(lines[1], "12345");
    }

    #[test]
    fn widget_truncate_content_shorter_line_unchanged() {
        let mut w = Widget::new("id", "hi", 1);
        w.truncate_content(80);
        assert_eq!(w.content, "hi");
    }
}

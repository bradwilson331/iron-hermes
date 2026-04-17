//! KeybindingRegistry — collects, matches, and formats keyboard shortcut help.
//!
//! Phase 22.1 Plan 01 Task 2.

use crate::tui::extension::{KeyContext, Keybinding, TuiExtension};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// ---------------------------------------------------------------------------
// KeybindingRegistry
// ---------------------------------------------------------------------------

/// Collects keybindings from all registered TUI extensions and provides
/// context-aware matching and help generation.
pub struct KeybindingRegistry {
    bindings: Vec<Keybinding>,
}

impl KeybindingRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { bindings: Vec::new() }
    }

    /// Collect keybindings from all extensions into a single registry.
    pub fn register_from_extensions(extensions: &[Box<dyn TuiExtension>]) -> Self {
        let bindings = extensions.iter().flat_map(|ext| ext.keybindings()).collect();
        Self { bindings }
    }

    /// Return the `action_name` of the first binding that matches `key` in
    /// the given `context`.
    ///
    /// A binding matches when:
    /// - `binding.key.code == key.code`
    /// - `binding.key.modifiers == key.modifiers`
    /// - The binding's `when` context is compatible:
    ///   - `KeyContext::Idle` matches `Idle` context
    ///   - `KeyContext::InFlight` matches `InFlight` context
    ///   - `KeyContext::Always` matches both
    pub fn match_key(&self, key: &KeyEvent, context: &KeyContext) -> Option<&str> {
        self.bindings.iter().find(|b| {
            b.key.code == key.code
                && b.key.modifiers == key.modifiers
                && context_matches(&b.when, context)
        }).map(|b| b.action_name.as_str())
    }

    /// Return tuples of `(key_display, description, context)` for all registered
    /// bindings, suitable for inclusion in /help output.
    pub fn help_entries(&self) -> Vec<(String, &str, &KeyContext)> {
        self.bindings
            .iter()
            .map(|b| (format_key_display(&b.key), b.description.as_str(), &b.when))
            .collect()
    }
}

impl Default for KeybindingRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `true` if a binding with context `binding_ctx` is active in
/// the current `active_ctx`.
fn context_matches(binding_ctx: &KeyContext, active_ctx: &KeyContext) -> bool {
    match binding_ctx {
        KeyContext::Always => true,
        KeyContext::Idle => *active_ctx == KeyContext::Idle,
        KeyContext::InFlight => *active_ctx == KeyContext::InFlight,
    }
}

/// Convert a `KeyEvent` into a human-readable key display string.
///
/// Examples: `"Ctrl+T"`, `"Alt+X"`, `"F1"`, `"Enter"`, `"Esc"`, `"Tab"`,
/// `"Backspace"`, `"Ctrl+Alt+D"`.
pub fn format_key_display(key: &KeyEvent) -> String {
    let mut parts: Vec<&str> = Vec::new();

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt");
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("Shift");
    }

    let key_str: String = match key.code {
        KeyCode::Char(c) => c.to_uppercase().to_string(),
        KeyCode::F(n) => format!("F{}", n),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "Shift+Tab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Insert => "Insert".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Null => "Null".to_string(),
        _ => format!("{:?}", key.code),
    };

    if parts.is_empty() {
        key_str
    } else {
        format!("{}+{}", parts.join("+"), key_str)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::extension::{KeyContext, Keybinding, TuiExtension};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        make_key(code, KeyModifiers::CONTROL)
    }

    fn plain_key(code: KeyCode) -> KeyEvent {
        make_key(code, KeyModifiers::NONE)
    }

    fn make_binding(code: KeyCode, modifiers: KeyModifiers, when: KeyContext, action: &str) -> Keybinding {
        Keybinding {
            key: make_key(code, modifiers),
            description: format!("Action: {}", action),
            when,
            action_name: action.to_string(),
        }
    }

    struct ExtWithBindings {
        bindings: Vec<Keybinding>,
    }

    impl TuiExtension for ExtWithBindings {
        fn name(&self) -> &str { "ext_with_bindings" }
        fn keybindings(&self) -> Vec<Keybinding> { self.bindings.clone() }
    }

    struct NoOpExt;
    impl TuiExtension for NoOpExt {
        fn name(&self) -> &str { "noop" }
    }

    // --- Construction ---

    #[test]
    fn new_creates_empty_registry() {
        let r = KeybindingRegistry::new();
        assert!(r.bindings.is_empty());
    }

    // --- register_from_extensions ---

    #[test]
    fn register_from_extensions_collects_bindings() {
        let ext: Box<dyn TuiExtension> = Box::new(ExtWithBindings {
            bindings: vec![
                make_binding(KeyCode::Char('t'), KeyModifiers::CONTROL, KeyContext::Idle, "toggle"),
                make_binding(KeyCode::F(1), KeyModifiers::NONE, KeyContext::Always, "help"),
            ],
        });
        let registry = KeybindingRegistry::register_from_extensions(&[ext]);
        assert_eq!(registry.bindings.len(), 2);
    }

    #[test]
    fn register_from_no_extensions_returns_empty() {
        let registry = KeybindingRegistry::register_from_extensions(&[]);
        assert!(registry.bindings.is_empty());
    }

    // --- match_key ---

    #[test]
    fn match_key_idle_context_matches_idle_binding() {
        let mut registry = KeybindingRegistry::new();
        registry.bindings.push(make_binding(
            KeyCode::Char('t'), KeyModifiers::CONTROL, KeyContext::Idle, "toggle",
        ));
        let result = registry.match_key(&ctrl_key(KeyCode::Char('t')), &KeyContext::Idle);
        assert_eq!(result, Some("toggle"));
    }

    #[test]
    fn match_key_idle_context_matches_always_binding() {
        let mut registry = KeybindingRegistry::new();
        registry.bindings.push(make_binding(
            KeyCode::F(1), KeyModifiers::NONE, KeyContext::Always, "help",
        ));
        let result = registry.match_key(&plain_key(KeyCode::F(1)), &KeyContext::Idle);
        assert_eq!(result, Some("help"));
    }

    #[test]
    fn match_key_idle_context_does_not_match_inflight_binding() {
        let mut registry = KeybindingRegistry::new();
        registry.bindings.push(make_binding(
            KeyCode::Char('c'), KeyModifiers::CONTROL, KeyContext::InFlight, "cancel",
        ));
        let result = registry.match_key(&ctrl_key(KeyCode::Char('c')), &KeyContext::Idle);
        assert_eq!(result, None);
    }

    #[test]
    fn match_key_inflight_context_matches_inflight_binding() {
        let mut registry = KeybindingRegistry::new();
        registry.bindings.push(make_binding(
            KeyCode::Char('c'), KeyModifiers::CONTROL, KeyContext::InFlight, "cancel",
        ));
        let result = registry.match_key(&ctrl_key(KeyCode::Char('c')), &KeyContext::InFlight);
        assert_eq!(result, Some("cancel"));
    }

    #[test]
    fn match_key_inflight_context_matches_always_binding() {
        let mut registry = KeybindingRegistry::new();
        registry.bindings.push(make_binding(
            KeyCode::F(1), KeyModifiers::NONE, KeyContext::Always, "help",
        ));
        let result = registry.match_key(&plain_key(KeyCode::F(1)), &KeyContext::InFlight);
        assert_eq!(result, Some("help"));
    }

    #[test]
    fn match_key_inflight_context_does_not_match_idle_only_binding() {
        let mut registry = KeybindingRegistry::new();
        registry.bindings.push(make_binding(
            KeyCode::Char('t'), KeyModifiers::CONTROL, KeyContext::Idle, "toggle",
        ));
        let result = registry.match_key(&ctrl_key(KeyCode::Char('t')), &KeyContext::InFlight);
        assert_eq!(result, None);
    }

    #[test]
    fn match_key_returns_none_when_no_binding_matches() {
        let registry = KeybindingRegistry::new();
        let result = registry.match_key(&plain_key(KeyCode::Enter), &KeyContext::Idle);
        assert_eq!(result, None);
    }

    // --- help_entries ---

    #[test]
    fn help_entries_returns_descriptions_for_all_bindings() {
        let mut registry = KeybindingRegistry::new();
        registry.bindings.push(make_binding(
            KeyCode::Char('t'), KeyModifiers::CONTROL, KeyContext::Idle, "toggle",
        ));
        registry.bindings.push(make_binding(
            KeyCode::F(1), KeyModifiers::NONE, KeyContext::Always, "help",
        ));
        let entries = registry.help_entries();
        assert_eq!(entries.len(), 2);
        // Check key display is formatted
        assert!(entries[0].0.contains("Ctrl"));
        // Check descriptions present
        assert!(!entries[0].1.is_empty());
        assert!(!entries[1].1.is_empty());
    }

    // --- format_key_display ---

    #[test]
    fn format_key_display_ctrl_t() {
        let key = ctrl_key(KeyCode::Char('t'));
        assert_eq!(format_key_display(&key), "Ctrl+T");
    }

    #[test]
    fn format_key_display_alt_x() {
        let key = make_key(KeyCode::Char('x'), KeyModifiers::ALT);
        assert_eq!(format_key_display(&key), "Alt+X");
    }

    #[test]
    fn format_key_display_f1() {
        let key = plain_key(KeyCode::F(1));
        assert_eq!(format_key_display(&key), "F1");
    }

    #[test]
    fn format_key_display_enter() {
        let key = plain_key(KeyCode::Enter);
        assert_eq!(format_key_display(&key), "Enter");
    }

    #[test]
    fn format_key_display_esc() {
        let key = plain_key(KeyCode::Esc);
        assert_eq!(format_key_display(&key), "Esc");
    }
}

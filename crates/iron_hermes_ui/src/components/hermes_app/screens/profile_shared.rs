//! Phase 50.1 Plan 02 (D-10): shared profile-component module root. Kanban's
//! profile create wizard, detail drawer and switcher (all UAT-verified from
//! Phase 47.4) were relocated here so both the Kanban screen and the Agents
//! screen consume ONE implementation — never two forked copies. The old
//! `screens/kanban/{wizard,profile_drawer,profile_switcher}.rs` paths remain
//! as thin re-export shims so Kanban's existing import sites keep resolving
//! unchanged.
//!
//! Task 2 of this plan adds `ProfileDialogContext`, the seam that lets the
//! same components render bot-flavored copy on the Agents screen and
//! kanban-flavored copy on the Kanban board.

pub mod advanced;
pub mod create_dialog;
pub mod edit_dialog;
pub mod secrets_source_picker;
pub mod switcher;

/// Phase 50.1 Plan 02 (D-10): selects which caller is rendering the shared
/// profile dialogs — `CreateProfileWizard` and `ProfileDetailDrawer` both
/// take this as a `context` prop. Defaults to `Kanban` so an omitted prop
/// (any pre-lift call site that hasn't been updated yet) preserves the
/// pre-lift kanban behavior exactly.
///
#[allow(dead_code)] // Bot variant + empty_roster_body consumed by a later plan's switcher-lift wiring; dead_code fires under --all-features (legacy-shell swaps the reachable root component, mirrors Plan 01's identical precedent)
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum ProfileDialogContext {
    Bot,
    #[default]
    Kanban,
}

impl ProfileDialogContext {
    /// The locked wizard title string for this context (UI-SPEC Copywriting
    /// Contract). The Kanban string is the 47.4-locked title — never altered
    /// by this lift.
    #[allow(dead_code)] // used in CreateProfileWizard rsx!; dead_code fires under --all-features
    pub fn wizard_title(&self) -> &'static str {
        match self {
            ProfileDialogContext::Bot => "Create Bot",
            ProfileDialogContext::Kanban => "Create Kanban Profile",
        }
    }

    /// The locked empty-roster-state body copy for this context. The Kanban
    /// string is byte-identical to the string the pre-lift switcher rendered
    /// (`profile_switcher.rs`'s "No profiles yet." body).
    #[allow(dead_code)] // consumed by a later plan's switcher-lift wiring
    pub fn empty_roster_body(&self) -> &'static str {
        match self {
            ProfileDialogContext::Bot => {
                "Create one to give it its own look, chat, and schedule — press + NEW AGENT above."
            }
            ProfileDialogContext::Kanban => {
                "Create one to give a kanban worker its own config.yaml and .env — press + NEW PROFILE above."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_wizard_title_is_create_bot() {
        assert_eq!(ProfileDialogContext::Bot.wizard_title(), "Create Bot");
    }

    #[test]
    fn kanban_wizard_title_is_create_kanban_profile() {
        assert_eq!(
            ProfileDialogContext::Kanban.wizard_title(),
            "Create Kanban Profile"
        );
    }

    #[test]
    fn bot_empty_roster_body_matches_bot_roster_copy() {
        assert_eq!(
            ProfileDialogContext::Bot.empty_roster_body(),
            "Create one to give it its own look, chat, and schedule — press + NEW AGENT above."
        );
    }

    #[test]
    fn kanban_empty_roster_body_is_byte_identical_to_pre_lift_switcher_copy() {
        assert_eq!(
            ProfileDialogContext::Kanban.empty_roster_body(),
            "Create one to give a kanban worker its own config.yaml and .env — press + NEW PROFILE above."
        );
    }

    #[test]
    fn default_context_is_kanban_so_an_omitted_prop_preserves_pre_lift_behavior() {
        assert_eq!(ProfileDialogContext::default(), ProfileDialogContext::Kanban);
    }

    #[test]
    fn context_derives_clone_copy_and_partial_eq() {
        let ctx = ProfileDialogContext::Bot;
        let copied = ctx;
        assert_eq!(ctx, copied);
        // `Clone::clone` via reference (not `Copy`'s implicit path) — proves
        // the derive without tripping clippy's clone-on-copy lint.
        let cloned = Clone::clone(&ctx);
        assert_eq!(ctx, cloned);
    }
}

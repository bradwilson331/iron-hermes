//! Phase 50.2 Plan 05 (D-21/D-10/D-11, UI-SPEC Component Inventory §7): the
//! `Group Chat Settings` drawer — makes D-21's orchestration tunables real
//! and editable from the web app.
//!
//! **Mount idiom (D-10, matches `ProfileDetailDrawer`).** Mounted
//! unconditionally at every call site; every hook registers on every
//! render, and the component renders nothing until its `open: Signal<bool>`
//! prop is `true`. The `is_open` snapshot is read once, BEFORE the early
//! return, so hook ordering never depends on whether the drawer is
//! currently visible.
//!
//! **Resource idiom (D-10).** `use_resource` with a `refresh_tick`
//! sync-prefix — never `use_server_future` + `.restart()`. The fetched
//! record seeds the draft exactly once (`seeded` flag) so a resource
//! refire never clobbers an operator's in-progress, unsaved edits.
//!
//! **No context provider (D-10).** `open` is a plain prop, threaded down
//! from whichever screen mounts this component — this file opens no
//! context of its own. This plan wires the ONE entry point inside
//! `group_chat_workspace.rs`'s room header; plan 06 adds the second entry
//! point (the roster section header) when it owns `bot_roster.rs`, mounting
//! its OWN `settings_open: Signal<bool>` and its OWN instance of this same
//! component — two entry points to the same persisted record, not a shared
//! signal.
//!
//! **Client-side validation is a convenience, never authoritative.**
//! [`group_settings_field_errors`] mirrors
//! `group_settings_api::clamp_group_settings`'s bounds exactly so an
//! out-of-range value disables SAVE before a round trip — but
//! `save_group_chat_settings` re-validates through the real
//! `clamp_group_settings` server-side and that remains the authoritative
//! gate. A divergence between the two is the bug the boundary-value test
//! below exists to catch, not a state either validator is allowed to drift
//! into silently.
//!
//! **A failed SAVE never discards the operator's edits.** `draft` is only
//! ever overwritten by a SUCCESSFUL save (echoing the server's returned
//! record) or an explicit RESET TO DEFAULTS click — never by an error path.
//!
//! Signal-borrow discipline (clippy.toml): every value used in the `rsx!`
//! block is extracted via `.read()` BEFORE it; `on_save_click`'s read of
//! `draft` happens synchronously at click time (mirrors
//! `group_chat_workspace.rs::submit_room_send`'s own shape) — the borrow is
//! dropped via `.clone()` immediately, never held across the `spawn`ed
//! `.await`.

use crate::protocol::GroupChatSettings;
use crate::server::group_settings_api::{load_group_chat_settings, save_group_chat_settings};
use dioxus::prelude::*;

/// Phase 50.2 Plan 05: whether the draft differs from the currently-saved
/// record in any of its five fields. Pure and disk/DOM-I/O-free — delegates
/// to `GroupChatSettings`'s own derived `PartialEq`, which compares every
/// field.
#[allow(dead_code)] // see group_settings module's own reachability note below
pub(crate) fn settings_form_is_dirty(saved: &GroupChatSettings, draft: &GroupChatSettings) -> bool {
    saved != draft
}

/// Phase 50.2 Plan 05: client-side mirror of
/// `group_settings_api::clamp_group_settings`'s exact bounds — a UX
/// convenience only, never the authoritative gate (see module doc). Returns
/// one `(field, reason)` pair per violated rule, in the same field order
/// the server-side clamp checks them, so a boundary-value test can assert
/// the two validators agree without either being the source of truth for
/// the other.
#[allow(dead_code)] // see group_settings module's own reachability note below
pub(crate) fn group_settings_field_errors(s: &GroupChatSettings) -> Vec<(&'static str, String)> {
    let mut errors = Vec::new();
    if s.max_rounds < 1 || s.max_rounds > 10 {
        errors.push(("max_rounds", "must be between 1 and 10".to_string()));
    }
    if s.max_messages < 1 || s.max_messages > 100 {
        errors.push(("max_messages", "must be between 1 and 100".to_string()));
    }
    if s.history_limit < 1 || s.history_limit > 200 {
        errors.push(("history_limit", "must be between 1 and 200".to_string()));
    }
    if s.min_members < 2 {
        errors.push(("min_members", "must be at least 2".to_string()));
    }
    if s.max_members > 6 {
        errors.push(("max_members", "must be at most 6".to_string()));
    }
    if s.min_members > s.max_members {
        errors.push(("member_bounds", "MIN must not exceed MAX".to_string()));
    }
    errors
}

/// Phase 50.2 Plan 05: the Group Chat Settings drawer. Mounted
/// unconditionally at the call site; `open` decides visibility.
///
/// Under `--all-features`, `legacy-shell` swaps the reachable root
/// component to `WarpHermes`, leaving this component (and the pure fns
/// above, only ever called from its own `rsx!`) unreferenced from `main()`
/// for dead-code-lint purposes — even though both are live in the default
/// (non-legacy-shell) build. Same pre-existing pattern as
/// `bot_roster/delete_confirm.rs`'s `confirm_input_matches` and this
/// phase's own `group_chat_workspace.rs` precedent.
#[allow(dead_code)]
#[component]
pub fn GroupSettingsDrawer(open: Signal<bool>) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E —
    // mirrors ProfileDetailDrawer's own early-return-after-hooks shape).
    let refresh_tick: Signal<u32> = use_signal(|| 0);
    let settings_resource = use_resource(move || {
        let _tick = refresh_tick();
        async move { load_group_chat_settings().await }
    });

    let mut saved_settings: Signal<Option<GroupChatSettings>> = use_signal(|| None);
    let mut draft: Signal<GroupChatSettings> = use_signal(GroupChatSettings::default);
    let mut seeded: Signal<bool> = use_signal(|| false);
    let mut saving: Signal<bool> = use_signal(|| false);
    let mut save_error: Signal<Option<String>> = use_signal(|| None);
    let mut just_saved: Signal<bool> = use_signal(|| false);

    // Seed the draft from the loaded record EXACTLY ONCE — a resource
    // refire (if `refresh_tick` is ever bumped in the future) must never
    // clobber an operator's in-progress, unsaved edits.
    use_effect(move || {
        if let Some(Ok(loaded)) = settings_resource() {
            if !*seeded.peek() {
                saved_settings.set(Some(loaded.clone()));
                draft.set(loaded);
                seeded.set(true);
            }
        }
    });

    // Snapshot BEFORE any conditional return — clippy.toml signal-borrow
    // discipline (no GenerationalRef held across rsx!).
    let is_open = *open.read();
    if !is_open {
        // Hooks above stay registered across opens/closes; only the
        // visible subtree is skipped.
        return rsx! {};
    }

    // ---- Derived values (read BEFORE rsx!). ----
    let is_loading = settings_resource().is_none();
    let load_failed = matches!(settings_resource(), Some(Err(_)));
    let draft_val = draft.read().clone();
    let saved_val = saved_settings.read().clone();
    let is_saving = *saving.read();
    let just_saved_val = *just_saved.read();
    let save_error_val = save_error.read().clone();

    let errors = group_settings_field_errors(&draft_val);
    let field_error = |field: &str| -> Option<String> {
        errors
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, r)| r.clone())
    };
    let max_rounds_error = field_error("max_rounds");
    let max_messages_error = field_error("max_messages");
    let history_limit_error = field_error("history_limit");
    let min_members_error = field_error("min_members");
    let max_members_error = field_error("max_members");
    let member_bounds_error = field_error("member_bounds");

    let is_dirty = saved_val
        .as_ref()
        .map(|saved| settings_form_is_dirty(saved, &draft_val))
        .unwrap_or(false);
    let can_save = is_dirty && errors.is_empty() && !is_saving;

    let on_reset_click = move |_| {
        draft.set(GroupChatSettings::default());
        just_saved.set(false);
        save_error.set(None);
    };

    let on_save_click = move |_| {
        // Read synchronously at click time, `.clone()`d immediately — the
        // borrow never spans the spawned `.await` below (mirrors
        // `group_chat_workspace.rs::submit_room_send`'s own shape).
        let to_save = draft.read().clone();
        if !group_settings_field_errors(&to_save).is_empty() {
            return;
        }
        saving.set(true);
        save_error.set(None);
        just_saved.set(false);
        let mut saving_sig = saving;
        let mut save_error_sig = save_error;
        let mut saved_settings_sig = saved_settings;
        let mut just_saved_sig = just_saved;
        let mut draft_sig = draft;
        spawn(async move {
            match save_group_chat_settings(to_save).await {
                Ok(saved) => {
                    // Fresh `.set()` calls only, acquired after the await
                    // resolves — never a write-lock guard held across it.
                    saving_sig.set(false);
                    draft_sig.set(saved.clone());
                    saved_settings_sig.set(Some(saved));
                    just_saved_sig.set(true);
                }
                Err(e) => {
                    // The draft is deliberately left UNTOUCHED here — a
                    // failed save must never silently discard what the
                    // operator typed.
                    saving_sig.set(false);
                    save_error_sig.set(Some(format!("{e}")));
                }
            }
        });
    };

    rsx! {
        aside {
            class: "kn-drawer",
            "data-open": "true",
            role: "complementary",
            "aria-label": "Group Chat Settings",
            "aria-modal": "false",
            onkeydown: move |event| {
                if event.key() == Key::Escape {
                    open.set(false);
                }
            },
            div { class: "kn-drawer-header",
                div { class: "kn-drawer-header-row",
                    h2 { class: "kn-drawer-title", "Group Chat Settings" }
                    button {
                        class: "kn-drawer-close",
                        "aria-label": "Close group chat settings",
                        onclick: move |_| open.set(false),
                        "✕"
                    }
                }
            }
            if is_loading {
                div { class: "kn-drawer-section",
                    div { class: "kn-drawer-loading", "Loading…" }
                }
            } else if load_failed {
                div { class: "kn-drawer-section",
                    div { class: "kn-modal-error", "Could not load group chat settings." }
                }
            } else {
                div { class: "kn-drawer-section",
                    div { class: "kn-settings-field",
                        label { class: "kn-modal-label", "MAX ROUNDS" }
                        input {
                            class: "kn-modal-input",
                            r#type: "number",
                            value: "{draft_val.max_rounds}",
                            oninput: move |evt| {
                                draft.write().max_rounds = evt.value().parse().unwrap_or(0);
                                just_saved.set(false);
                            },
                        }
                        div { class: "kn-modal-hint--info", "Upstream default: 3" }
                        if let Some(err) = max_rounds_error {
                            div { class: "kn-modal-error", "{err}" }
                        }
                    }
                    div { class: "kn-settings-field",
                        label { class: "kn-modal-label", "MAX MESSAGES PER TURN" }
                        input {
                            class: "kn-modal-input",
                            r#type: "number",
                            value: "{draft_val.max_messages}",
                            oninput: move |evt| {
                                draft.write().max_messages = evt.value().parse().unwrap_or(0);
                                just_saved.set(false);
                            },
                        }
                        div { class: "kn-modal-hint--info", "Upstream default: 10" }
                        if let Some(err) = max_messages_error {
                            div { class: "kn-modal-error", "{err}" }
                        }
                    }
                    div { class: "kn-settings-field",
                        label { class: "kn-modal-label", "HISTORY LIMIT" }
                        input {
                            class: "kn-modal-input",
                            r#type: "number",
                            value: "{draft_val.history_limit}",
                            oninput: move |evt| {
                                draft.write().history_limit = evt.value().parse().unwrap_or(0);
                                just_saved.set(false);
                            },
                        }
                        div {
                            class: "kn-modal-hint--info",
                            "How many recent room messages each member sees per turn. Upstream default: 24."
                        }
                        if let Some(err) = history_limit_error {
                            div { class: "kn-modal-error", "{err}" }
                        }
                    }
                    div { class: "kn-settings-field",
                        label { class: "kn-modal-label", "MEMBER LIMIT" }
                        div { style: "display: flex; gap: var(--sp-2);",
                            input {
                                class: "kn-modal-input",
                                r#type: "number",
                                value: "{draft_val.min_members}",
                                oninput: move |evt| {
                                    draft.write().min_members = evt.value().parse().unwrap_or(0);
                                    just_saved.set(false);
                                },
                            }
                            input {
                                class: "kn-modal-input",
                                r#type: "number",
                                value: "{draft_val.max_members}",
                                oninput: move |evt| {
                                    draft.write().max_members = evt.value().parse().unwrap_or(0);
                                    just_saved.set(false);
                                },
                            }
                        }
                        div { class: "kn-modal-hint--info", "Upstream defaults: 2–6" }
                        if let Some(err) = min_members_error {
                            div { class: "kn-modal-error", "{err}" }
                        }
                        if let Some(err) = max_members_error {
                            div { class: "kn-modal-error", "{err}" }
                        }
                        if let Some(err) = member_bounds_error {
                            div { class: "kn-modal-error", "{err}" }
                        }
                    }
                    div { class: "kn-modal-hint--info", "Each member's turn is capped at 180s (fixed)." }
                }
                div { class: "kn-drawer-section",
                    button {
                        class: "kn-action-btn kn-settings-reset",
                        onclick: on_reset_click,
                        "RESET TO DEFAULTS"
                    }
                    button {
                        class: "kn-modal-btn kn-modal-btn--submit",
                        disabled: !can_save,
                        onclick: on_save_click,
                        if is_saving { "SAVING…" } else { "SAVE" }
                    }
                    if just_saved_val {
                        div { class: "kn-modal-hint--info", "Saved" }
                    }
                    if let Some(err) = save_error_val {
                        div { class: "kn-modal-error", "{err}" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod settings_form_is_dirty_tests {
    use super::*;

    fn base() -> GroupChatSettings {
        GroupChatSettings::default()
    }

    #[test]
    fn identical_records_are_not_dirty() {
        assert!(!settings_form_is_dirty(&base(), &base()));
    }

    #[test]
    fn max_rounds_difference_is_dirty() {
        let draft = GroupChatSettings {
            max_rounds: 1,
            ..base()
        };
        assert!(settings_form_is_dirty(&base(), &draft));
    }

    #[test]
    fn max_messages_difference_is_dirty() {
        let draft = GroupChatSettings {
            max_messages: 5,
            ..base()
        };
        assert!(settings_form_is_dirty(&base(), &draft));
    }

    #[test]
    fn history_limit_difference_is_dirty() {
        let draft = GroupChatSettings {
            history_limit: 12,
            ..base()
        };
        assert!(settings_form_is_dirty(&base(), &draft));
    }

    #[test]
    fn min_members_difference_is_dirty() {
        let draft = GroupChatSettings {
            min_members: 3,
            ..base()
        };
        assert!(settings_form_is_dirty(&base(), &draft));
    }

    #[test]
    fn max_members_difference_is_dirty() {
        let draft = GroupChatSettings {
            max_members: 4,
            ..base()
        };
        assert!(settings_form_is_dirty(&base(), &draft));
    }
}

#[cfg(test)]
mod group_settings_field_errors_tests {
    use super::*;

    #[test]
    fn default_record_has_no_errors() {
        assert!(group_settings_field_errors(&GroupChatSettings::default()).is_empty());
    }

    #[test]
    fn max_rounds_zero_is_flagged() {
        let s = GroupChatSettings {
            max_rounds: 0,
            ..GroupChatSettings::default()
        };
        let errors = group_settings_field_errors(&s);
        assert!(errors.iter().any(|(f, _)| *f == "max_rounds"));
    }

    #[test]
    fn max_members_seven_is_flagged() {
        let s = GroupChatSettings {
            max_members: 7,
            ..GroupChatSettings::default()
        };
        let errors = group_settings_field_errors(&s);
        assert!(errors.iter().any(|(f, _)| *f == "max_members"));
    }

    /// `<behavior>` boundary-agreement requirement: the client-side check
    /// and the server-side `clamp_group_settings` must agree at every
    /// boundary value — a divergence between the two is the bug this test
    /// exists to catch, not a state either validator may drift into.
    #[cfg(feature = "server")]
    #[test]
    fn client_side_check_agrees_with_server_clamp_at_every_boundary_value() {
        use crate::server::group_settings_api::clamp_group_settings;

        let cases: Vec<GroupChatSettings> = vec![
            GroupChatSettings {
                max_rounds: 0,
                ..GroupChatSettings::default()
            },
            GroupChatSettings {
                max_rounds: 1,
                ..GroupChatSettings::default()
            },
            GroupChatSettings {
                max_rounds: 10,
                ..GroupChatSettings::default()
            },
            GroupChatSettings {
                max_rounds: 11,
                ..GroupChatSettings::default()
            },
            GroupChatSettings {
                min_members: 1,
                ..GroupChatSettings::default()
            },
            GroupChatSettings {
                min_members: 2,
                ..GroupChatSettings::default()
            },
            GroupChatSettings {
                max_members: 6,
                ..GroupChatSettings::default()
            },
            GroupChatSettings {
                max_members: 7,
                ..GroupChatSettings::default()
            },
        ];

        for case in cases {
            let client_valid = group_settings_field_errors(&case).is_empty();
            let server_valid = clamp_group_settings(&case).is_ok();
            assert_eq!(
                client_valid, server_valid,
                "client/server validators disagree for {case:?}"
            );
        }
    }
}

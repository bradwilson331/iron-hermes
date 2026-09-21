//! Phase 50.4 Plan 03 (D-06/D-07) introduced `ModelPickerField` as a capped,
//! open-on-click model popup replacing the native `<input list>` +
//! `<datalist>` render target `models.rs`'s `ProviderModelCascade` shipped
//! as a Phase 49.4 hotfix, and described it as a presentation-only
//! replacement whose Rust-side derivation stayed with the caller. Phase
//! 50.5 (D-17/D-18/D-19) makes that no longer true: this component now OWNS
//! the text filter, the display cap, and the edited-state decision for all
//! three surfaces it is mounted on. It used to take an already-filtered,
//! already-capped `options` list as a prop; the three call sites had each
//! grown their own copy of the same filter block (one of them its own cap
//! constant too), and the collision between a stale seeded id and that
//! caller-side filter is what shipped the 2026-09-08 operator-hit defect
//! this phase exists to close (see `unedited_field_shows_the_full_provider_list`
//! below). Callers now pass the FULL, unfiltered `all_options` plus the
//! `seeded` value the field was initialized from; this component derives
//! everything else.
//!
//! Behavioural-change record (D-18): before Phase 50.4 D-06 this field was a
//! plain text input whose contents did not filter anything — the model list
//! was a native `<datalist>`, filtered by the browser, not by this crate.
//! D-06 replaced that with a Rust-side filter but never recorded the change
//! in behaviour: a field showing a value no longer showed the FULL list on
//! open, it showed a list already narrowed to whatever text happened to be
//! sitting in the input — including a stale id the operator never typed.
//! This module doc comment is that missing record.
//!
//! Mounted on three surfaces: the Models screen's seven cascades, the
//! Providers default-model field, and the profile drawer picker.

use dioxus::prelude::*;

/// Phase 50.5 (D-17/D-18/D-19): the workspace's only model-picker display
/// cap. Replaces the `models.rs` module-scope cap constant and
/// `edit_dialog.rs`'s locally redeclared copy of the same constant — both
/// deleted in this phase's Task 3 so exactly one cap exists. The value
/// itself is unchanged from 50.4 D-07: `50.4` D-07's performance
/// constraint (never render more than this many popup rows regardless of
/// catalog size) survives by construction, not by intent.
pub(crate) const MODEL_PICKER_CAP: usize = 50;

/// Phase 50.5 (D-19/UI-SPEC E6): the character budget the search-affordance
/// row's echoed typed text is truncated to before render, so operator text
/// of any length renders as exactly one line and cannot grow the popup
/// header (UI-SPEC E6 overflow).
const TYPED_ECHO_BUDGET: usize = 32;

/// Phase 50.5 (D-17): whether the operator has genuinely edited the field
/// away from the value it was seeded with. `dirty` alone is not enough — an
/// operator can type the seeded value back in, and text equal to the seeded
/// value is not an edit whatever the flag says (both sides are trimmed: the
/// seeded value arrives from config, the typed value from an input event).
///
/// `cfg_attr(not(wasm), allow(dead_code))`: called only from
/// `ModelPickerField` (web-live component tree); native `--all-features`
/// bin sees the component tree as unreachable (server entry).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn effective_dirty(dirty: bool, typed: &str, seeded: &str) -> bool {
    dirty && typed.trim() != seeded.trim()
}

/// Phase 50.5 (D-17/D-19): the single filter implementation for all three
/// picker surfaces. When the field is not effectively dirty, `all_options`
/// passes through UNTOUCHED — no text filter — which is the whole D-17 fix:
/// a stale seeded id sitting in the input must never narrow the list the
/// operator has not edited. Once effectively dirty, the matched set is
/// `all_options` filtered by a case-insensitive substring match, byte-
/// identical to the caller-side filter this replaces
/// (`id.to_ascii_lowercase().contains(&typed.trim().to_ascii_lowercase())`).
///
/// Returns `(displayed, total)`: `displayed` is the matched set capped to
/// `cap` rows; `total` is the matched count BEFORE capping, which is what
/// `cap_notice` needs to report a truthful "{shown}/{total}".
///
/// `cfg_attr(not(wasm), allow(dead_code))`: same rationale as
/// `effective_dirty` above.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn filter_options(
    all_options: &[String],
    typed: &str,
    seeded: &str,
    dirty: bool,
    cap: usize,
) -> (Vec<String>, usize) {
    let matched: Vec<&String> = if effective_dirty(dirty, typed, seeded) {
        let needle = typed.trim().to_ascii_lowercase();
        all_options
            .iter()
            .filter(|id| id.to_ascii_lowercase().contains(&needle))
            .collect()
    } else {
        all_options.iter().collect()
    };
    let total = matched.len();
    let displayed = matched.into_iter().take(cap).cloned().collect();
    (displayed, total)
}

/// UI-SPEC E5 empty-state copy — the sole occurrence of this literal in the
/// file; `empty_catalog_note` and its tests both reference this constant
/// rather than duplicating the string.
const EMPTY_CATALOG_NOTE: &str = "this provider returned no models — type an id to use it directly";

/// Phase 50.5 (D-18/UI-SPEC E5 empty): the free-text row shown when the
/// current provider's catalog is genuinely empty AND the operator has not
/// edited the field — distinct from a no-match result, which requires
/// typed text. `cfg_attr(not(wasm), allow(dead_code))`: same rationale as
/// `effective_dirty` above.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn empty_catalog_note(all_options: &[String], effective_dirty: bool) -> Option<String> {
    if all_options.is_empty() && !effective_dirty {
        Some(EMPTY_CATALOG_NOTE.to_string())
    } else {
        None
    }
}

/// Phase 50.5 (D-19/UI-SPEC E6): truncates the affordance row's echoed
/// typed text to `TYPED_ECHO_BUDGET` CHARACTERS (not bytes — a byte slice
/// can split a multi-byte model id and panic), appending an ellipsis when
/// truncated. `cfg_attr(not(wasm), allow(dead_code))`: same rationale as
/// `effective_dirty` above.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn truncate_echo(s: &str) -> String {
    if s.chars().count() <= TYPED_ECHO_BUDGET {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(TYPED_ECHO_BUDGET).collect();
        format!("{truncated}…")
    }
}

/// Phase 50.5 (D-18): a persistent search affordance, visible whenever the
/// popup is open regardless of whether the input already holds a value —
/// something the native `placeholder` attribute cannot do once text is
/// present. Reports the un-edited state as "showing {shown}/{total}", and
/// the edited state as a singular/plural match count against the
/// (truncated) typed text. `cfg_attr(not(wasm), allow(dead_code))`: same
/// rationale as `effective_dirty` above.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn search_affordance(
    shown: usize,
    total: usize,
    typed: &str,
    effective_dirty: bool,
) -> String {
    if !effective_dirty {
        format!("showing {shown}/{total} — type to search")
    } else {
        let echo = truncate_echo(typed.trim());
        if shown == 1 {
            format!("{shown} match for \"{echo}\"")
        } else {
            format!("{shown} matches for \"{echo}\"")
        }
    }
}

/// D-07 cap-affordance copy — pinned to the pre-existing string `models.rs`
/// already rendered beside its `<datalist>` (`"{shown}/{total} — type to
/// filter"`), just re-homed to the popup's last row. `None` when nothing is
/// hidden, so the caller renders no notice row at all.
///
/// `cfg_attr(not(wasm), allow(dead_code))`: called only from `ModelPickerField`
/// (web-live component tree); native `--all-features` bin sees the component
/// tree as unreachable (server entry) — same rationale as `compute_model_options`
/// in `models.rs`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn cap_notice(shown: usize, total: usize) -> Option<String> {
    if total > shown {
        Some(format!("{shown}/{total} — type to filter"))
    } else {
        None
    }
}

/// UI-SPEC Copywriting Contract, "Models/Providers/Drawer: model popup
/// empty-filter-result" — the free-text escape-hatch row shown when the
/// displayed set has nothing left after filtering AND the operator has
/// genuinely edited the field (D-18: widens WHEN this fires, not what it
/// says — the prepend-collision bug made this unreachable because the
/// caller-side filter always ran; gating on `effective_dirty` instead of
/// bare non-empty `typed` text is the fix). An un-edited, not-yet-loaded, or
/// empty field is NOT a no-match state, so this returns `None` then — the
/// popup renders nothing rather than a premature "no matches".
///
/// `cfg_attr(not(wasm), allow(dead_code))`: same rationale as `cap_notice`
/// above.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn no_match_hint(displayed: &[String], typed: &str, effective_dirty: bool) -> Option<String> {
    let typed = typed.trim();
    if effective_dirty && displayed.is_empty() && !typed.is_empty() {
        Some(format!(
            "No matches — press Enter to use \"{typed}\" as a new model id."
        ))
    } else {
        None
    }
}

/// Phase 50.4 Plan 03 (D-06/D-07) introduced this capped, open-on-click
/// model popup; Phase 50.5 (D-17/D-18/D-19) moved the filter, the cap, and
/// the edited-state decision INTO this component. `all_options` is the
/// caller's FULL, unfiltered catalog (`compute_model_options`'s raw output)
/// and `seeded` is the value the field was initialized from — this
/// component derives the filtered, capped `displayed` set itself via
/// `filter_options`, so it never iterates any collection other than
/// `displayed`, and cannot reintroduce the render-every-option freeze
/// `0ef32cc03` fixed regardless of catalog size.
///
/// `dirty` tracks whether the operator has typed into the field since it
/// was seeded; composed with `seeded`/`current` via `effective_dirty` so
/// typing the seeded value back in is not treated as an edit. A row
/// selection resets `dirty` to `false` — a freshly picked value is
/// conceptually a new seed, so reopening the popup shows the full list
/// again rather than one row narrowed by the id just chosen. There is
/// deliberately no reactive-effect hook resetting `dirty` on a `seeded`
/// change: `ProviderModelCascade` mounts this component with no `key:`, so
/// a provider change re-renders this same instance rather than remounting
/// it, and an effect that reads and writes in the same pass is this
/// crate's known infinite-render-loop shape. Composition via
/// `effective_dirty` already covers the "operator retypes the seeded
/// value" case without an effect.
///
/// Renders its own `.model-popup-wrap` (`position: relative`) so the
/// absolutely positioned `.model-popup` anchors to ITS OWN input rather than
/// to whatever distant positioned ancestor a host page happens to provide —
/// `.field-row` (Providers' host) is `display: grid` with no `position`
/// declared anywhere between it and the page root, so without this wrapper
/// the popup would resolve against an arbitrary ancestor or the viewport, a
/// render fault no build gate catches — the same failure class as commit
/// `81fc177a6` (passed every gate, rendered blank).
///
/// Blur-race handling: row selection commits via `onclick`, but a plain
/// `onblur` on the input would fire (and close the popup) BEFORE that click
/// registers, since mousedown-triggered focus loss happens before click.
/// Each row's `onmousedown` calls `prevent_default()`, which stops the
/// browser from moving focus off the input in the first place — no blur
/// fires, the click lands normally, and `onclick` closes the popup itself.
/// This is the standard "defer-blur-close" combobox technique (the plan's
/// other option — committing the selection directly on `onmousedown` — was
/// not used, so the row keeps ordinary `onclick` semantics).
#[component]
pub fn ModelPickerField(
    value: Signal<String>,
    all_options: Vec<String>,
    seeded: String,
    disabled: bool,
    placeholder: String,
    title: String,
    // Phase 50.5 CR-01 fix: fired ONLY on a confirmed row selection (the
    // popup row's `onclick`, below) — never on `oninput`, which fires on
    // every keystroke while the operator is still filtering/typing and
    // would make an unrelated sibling field (the CONTEXT WINDOW input on
    // `models.rs`/`providers.rs`) flicker or clear mid-type. A row click is
    // the one unambiguous "the operator picked a different model" event;
    // callers use it to reset state that describes the (provider, model)
    // pair rather than the field's own text. `#[props(default)]` keeps this
    // optional so `edit_dialog.rs`'s call site (no per-model sibling state
    // to reset) does not need to pass a no-op closure.
    #[props(default)] on_change: Option<EventHandler<String>>,
) -> Element {
    let mut open = use_signal(|| false);
    let mut dirty = use_signal(|| false);

    // Read the controlled value into an owned local before `rsx!` — no
    // `GenerationalRef` may be held across the macro (clippy.toml).
    let current = value.read().clone();
    let eff_dirty = effective_dirty(*dirty.read(), &current, &seeded);
    let (displayed, total) = filter_options(&all_options, &current, &seeded, *dirty.read(), MODEL_PICKER_CAP);
    let hint = no_match_hint(&displayed, &current, eff_dirty);
    let notice = cap_notice(displayed.len(), total);
    let empty_note = empty_catalog_note(&all_options, eff_dirty);
    let affordance = search_affordance(displayed.len(), total, &current, eff_dirty);
    let is_open = *open.read() && !disabled;

    rsx! {
        div { class: "model-popup-wrap",
            input {
                class: "voice-settings-select",
                value: "{current}",
                disabled,
                title: "{title}",
                placeholder: "{placeholder}",
                role: "combobox",
                aria_expanded: "{is_open}",
                oninput: move |evt| {
                    value.set(evt.value());
                    dirty.set(true);
                },
                onfocus: move |_| {
                    if !disabled {
                        open.set(true);
                    }
                },
                onclick: move |_| {
                    if !disabled {
                        open.set(true);
                    }
                },
                onkeydown: move |evt: KeyboardEvent| {
                    if evt.key() == Key::Escape {
                        open.set(false);
                    }
                },
                onblur: move |_| {
                    open.set(false);
                },
            }
            if is_open {
                div { class: "model-popup", role: "listbox",
                    div { class: "model-popup-note", "{affordance}" }
                    if let Some(empty_text) = empty_note {
                        div { class: "model-popup-note", "{empty_text}" }
                    } else if let Some(hint_text) = hint {
                        div { class: "model-popup-note", "{hint_text}" }
                    } else {
                        for id in displayed.iter() {
                            {
                                let id_owned = id.clone();
                                let is_active = id == &current;
                                rsx! {
                                    div {
                                        key: "{id_owned}",
                                        class: "model-popup-row",
                                        class: if is_active { "is-active" },
                                        title: "{id_owned}",
                                        role: "option",
                                        aria_selected: "{is_active}",
                                        onmousedown: move |evt| {
                                            evt.prevent_default();
                                        },
                                        onclick: move |_| {
                                            value.set(id_owned.clone());
                                            dirty.set(false);
                                            open.set(false);
                                            if let Some(handler) = on_change.as_ref() {
                                                handler.call(id_owned.clone());
                                            }
                                        },
                                        "{id_owned}"
                                    }
                                }
                            }
                        }
                    }
                    if let Some(notice_text) = notice {
                        div { class: "model-popup-note", "{notice_text}" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cap_notice, effective_dirty, empty_catalog_note, filter_options, no_match_hint,
        search_affordance, truncate_echo, EMPTY_CATALOG_NOTE,
    };

    #[test]
    fn test_cap_notice_shown_only_when_total_exceeds_shown() {
        assert_eq!(
            cap_notice(50, 430),
            Some("50/430 — type to filter".to_string())
        );
        assert_eq!(cap_notice(12, 12), None);
        assert_eq!(cap_notice(0, 0), None);
    }

    #[test]
    fn test_no_match_hint_requires_nonempty_typed_text() {
        let hint = no_match_hint(&[], "gpt-6-turbo", true);
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("gpt-6-turbo"));
        assert_eq!(no_match_hint(&[], "", true), None);
        assert_eq!(no_match_hint(&["a".to_string()], "a", true), None);
    }

    #[test]
    fn test_cap_notice_string_matches_the_shipped_format() {
        assert_eq!(cap_notice(50, 430).unwrap(), "50/430 — type to filter");
    }

    #[test]
    fn test_no_match_hint_string_matches_the_ui_spec_format() {
        assert_eq!(
            no_match_hint(&[], "gpt-6-turbo", true).unwrap(),
            "No matches — press Enter to use \"gpt-6-turbo\" as a new model id."
        );
    }

    fn fixture() -> Vec<String> {
        vec![
            "stale-x".to_string(),
            "prov-a".to_string(),
            "prov-b".to_string(),
        ]
    }

    /// Phase 50.5 (D-17): regression test for the 2026-09-08 operator-hit
    /// defect — switching to a provider that does not serve the currently
    /// assigned model narrowed the new provider's catalog to exactly one
    /// row (the stale assigned id) because `compute_model_options`'s
    /// unconditional prepend collided with the caller-side text filter,
    /// which used the field's own (un-edited) contents as the filter text.
    /// The field is un-edited (`dirty=false`) here, exactly as it was in
    /// the field report: the assigned id is absent from the new provider's
    /// list, and the operator has typed nothing.
    #[test]
    fn unedited_field_shows_the_full_provider_list() {
        let all = fixture();
        let (displayed, total) = filter_options(&all, "stale-x", "stale-x", false, 50);
        assert_eq!(displayed, all);
        assert_eq!(total, 3);
    }

    #[test]
    fn typed_text_filters_as_before() {
        let all = fixture();
        let (displayed, total) = filter_options(&all, "prov", "stale-x", true, 50);
        assert_eq!(displayed, vec!["prov-a".to_string(), "prov-b".to_string()]);
        assert_eq!(total, 2);
    }

    #[test]
    fn no_match_hint_is_reachable_once_typed() {
        assert!(no_match_hint(&[], "zzz", true).is_some());
        assert_eq!(no_match_hint(&[], "", false), None);
    }

    #[test]
    fn cap_notice_total_is_post_filter() {
        // 120 options total: 70 match "keep", 50 do not. Filtering to the
        // matching 70 and capping to 50 must report total=70 (post-filter,
        // pre-cap), not 120 (unfiltered) or 50 (post-cap).
        let mut all: Vec<String> = (0..70).map(|i| format!("keep-{i}")).collect();
        all.extend((0..50).map(|i| format!("drop-{i}")));
        assert_eq!(all.len(), 120);

        let (displayed, total) = filter_options(&all, "keep", "", true, 50);
        assert_eq!(displayed.len(), 50);
        assert_eq!(total, 70);
        assert_eq!(
            cap_notice(displayed.len(), total),
            Some("50/70 — type to filter".to_string())
        );

        let (displayed_uncapped, total_uncapped) = filter_options(&all, "", "", false, 50);
        assert_eq!(displayed_uncapped.len(), 50);
        assert_eq!(total_uncapped, 120);
        assert_eq!(
            cap_notice(displayed_uncapped.len(), total_uncapped),
            Some("50/120 — type to filter".to_string())
        );
    }

    #[test]
    fn empty_catalog_note_only_when_nothing_typed() {
        assert_eq!(
            empty_catalog_note(&[], false),
            Some(EMPTY_CATALOG_NOTE.to_string())
        );
        assert_eq!(empty_catalog_note(&[], true), None);
        assert_eq!(empty_catalog_note(&["a".to_string()], false), None);
    }

    #[test]
    fn search_affordance_states() {
        assert_eq!(
            search_affordance(50, 120, "", false),
            "showing 50/120 — type to search"
        );
        assert_eq!(
            search_affordance(3, 3, "gpt", true),
            "3 matches for \"gpt\""
        );
        assert_eq!(search_affordance(1, 1, "gp", true), "1 match for \"gp\"");
    }

    #[test]
    fn truncate_echo_bounds_the_echo() {
        let long = "a".repeat(100);
        let truncated = truncate_echo(&long);
        assert_eq!(truncated.chars().count(), 33);
        assert!(truncated.ends_with('…'));

        let short = "0123456789";
        assert_eq!(truncate_echo(short), short);

        // Multi-byte characters must not panic a byte-index slice.
        let multibyte = "文".repeat(40);
        let truncated_mb = truncate_echo(&multibyte);
        assert_eq!(truncated_mb.chars().count(), 33);
    }

    #[test]
    fn test_effective_dirty() {
        assert!(!effective_dirty(false, "opus-4", "opus-4"));
        assert!(!effective_dirty(true, "opus-4", "opus-4"));
        assert!(effective_dirty(true, "gpt", "opus-4"));
    }

    #[test]
    fn test_filter_options_uncapped_when_below_cap() {
        let all: Vec<String> = (0..120).map(|i| format!("model-{i}")).collect();
        let (displayed, total) = filter_options(&all, "", "", false, 50);
        assert_eq!(displayed.len(), 50);
        assert_eq!(total, 120);
    }

    /// Phase 50.5 CR-01 regression test. `parse_window_input`/`window_placeholder`
    /// are pure functions and cannot observe this class of defect — the bug
    /// lived entirely in a sibling signal's failure to reset when THIS
    /// component's selection changed, which only exists at the rendered
    /// component/event level (see CR-01 in `50.5-REVIEW.md`). This drives a
    /// real `VirtualDom` hosting `ModelPickerField` in isolation and asserts
    /// the actual mechanism the CR-01 fix depends on: a confirmed row click
    /// invokes `on_change` with the newly picked id (so `models.rs` /
    /// `providers.rs` can reset their CONTEXT WINDOW signal), while ordinary
    /// typing (`oninput`) — which fires on every keystroke while the
    /// operator is still filtering, before any model is actually chosen —
    /// does not. Element IDs below are asserted, not guessed: Dioxus assigns
    /// them deterministically for a fixed template/render sequence, and if a
    /// future markup change shifts them this test fails loudly (event
    /// delivered to the wrong id, `on_change` never observed) rather than
    /// silently passing on the wrong node — the same style used by
    /// `dioxus-core`'s own `tests/event_propagation.rs`.
    #[test]
    fn on_change_fires_on_confirmed_row_pick_not_on_typing() {
        use dioxus::dioxus_core::ElementId;
        use dioxus::prelude::*;
        use std::any::Any;
        use std::cell::RefCell;
        use std::rc::Rc;

        thread_local! {
            static LAST_CHANGE: RefCell<Option<String>> = const { RefCell::new(None) };
        }

        fn test_app() -> Element {
            let value = use_signal(String::new);
            rsx! {
                super::ModelPickerField {
                    value,
                    all_options: vec!["model-a".to_string(), "model-b".to_string()],
                    seeded: String::new(),
                    disabled: false,
                    placeholder: "".to_string(),
                    title: "".to_string(),
                    on_change: move |id: String| {
                        LAST_CHANGE.with(|c| *c.borrow_mut() = Some(id));
                    },
                }
            }
        }

        dioxus::html::set_event_converter(Box::new(dioxus::html::SerializedHtmlEventConverter));
        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_to_vec();

        let mouse_click = || {
            Event::new(
                Rc::new(dioxus::html::PlatformEventData::new(
                    Box::<dioxus::html::SerializedMouseData>::default(),
                )) as Rc<dyn Any>,
                true,
            )
        };

        // Typing into the input (ElementId(2), the field created by
        // `ModelPickerField`'s root template) must NOT fire `on_change` —
        // it only sets `value`/`dirty`, exactly as the review's fix
        // guidance requires (resetting on every keystroke would clear the
        // sibling CONTEXT WINDOW field while the operator is still
        // filtering, not yet having picked anything).
        let keyboard_input = Event::new(
            Rc::new(dioxus::html::PlatformEventData::new(
                Box::new(dioxus::html::SerializedFormData {
                    value: "model".to_string(),
                    values: Vec::new(),
                    valid: true,
                }),
            )) as Rc<dyn Any>,
            true,
        );
        dom.runtime()
            .handle_event("input", keyboard_input, ElementId(2));
        dom.render_immediate_to_vec();
        LAST_CHANGE.with(|c| assert_eq!(*c.borrow(), None, "typing must not fire on_change"));

        // Open the popup: a click on the input (ElementId(2)).
        dom.runtime().handle_event("click", mouse_click(), ElementId(2));
        dom.render_immediate_to_vec();
        LAST_CHANGE.with(|c| assert_eq!(*c.borrow(), None, "opening the popup must not fire on_change"));

        // Confirmed pick: click the "model-a" row (ElementId(6) — the
        // first popup row's own template root, per the fixed render
        // sequence this test pins).
        dom.runtime().handle_event("click", mouse_click(), ElementId(6));
        dom.render_immediate_to_vec();
        LAST_CHANGE.with(|c| {
            assert_eq!(
                c.borrow().as_deref(),
                Some("model-a"),
                "a confirmed row pick must fire on_change with the newly selected id"
            )
        });
    }
}

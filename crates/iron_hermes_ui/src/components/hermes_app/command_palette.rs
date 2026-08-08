//! Phase 41.1 Plan 06 (D-05) — Web `/` command palette, ported into
//! `hermes_app` (the active, non-`legacy-shell` root).
//!
//! This is an ADAPTATION of `shell_legacy::command_palette::CommandPalette`
//! (RESEARCH "Don't Hand-Roll" / Pitfall 6). The legacy component lives under
//! `#[cfg(feature = "legacy-shell")]` and is therefore NOT compiled in the
//! default build that mounts `HermesApp`, so it cannot be referenced directly
//! — it is ported here verbatim except for three deliberate changes:
//!
//!   1. The unified list now also renders `PaletteItem.section == "skill"`
//!      rows (UI-SPEC §A: commands UNION `SkillRegistry` entries, one flat
//!      filtered list), each carrying a DIM trailing `◆ skill` tag.
//!   2. The empty/no-match copy reads "No matching commands or skills."
//!      (UI-SPEC Copywriting Contract), replacing the legacy "no results".
//!   3. It is fed LIVE command + skill data by `ScreenChat`, not the static
//!      `PALETTE_ITEMS` mock the legacy shell wired.
//!
//! Geometry (`.wh-pal-overlay` centred ⌘K-style overlay), the keyboard model
//! (↑/↓ wrap, Enter = `on_pick`, live case-insensitive substring filter on
//! `cmd`+`label`), and the `.wh-pal-row` layout are reused verbatim. Esc is
//! handled by the parent (`ScreenChat`), preserving the legacy split where the
//! component itself does not own the open signal.

use crate::state::{PaletteItem, PaletteState, Personality};
use dioxus::prelude::*;

/// Web `/` command palette for `hermes_app` — adapted from
/// `shell_legacy::CommandPalette`. See the module doc for the three
/// deliberate divergences from the legacy component.
///
/// `selected` resets to 0 whenever `query` changes (via a `use_effect`
/// watching `query()`), matching the legacy risk-note behaviour.
// Consumed by ScreenChat (mod.rs mounts HermesApp; ScreenChat renders this).
// In a binary crate a `pub` item unreferenced under `--all-features`
// (legacy-shell mounts WarpHermes, leaving HermesApp/this unreferenced) reads
// as dead; live in the default build. Silence the cross-feature lint.
#[allow(dead_code)]
#[component]
pub fn CommandPalette(
    items: Vec<PaletteItem>,
    query: Signal<String>,
    open: ReadSignal<bool>,
    state: Signal<PaletteState>,
    on_pick: EventHandler<PaletteItem>,
) -> Element {
    if !open() {
        return rsx! {};
    }

    let mut selected = use_signal(|| 0_usize);

    // Reset selected when query changes (legacy risk note).
    use_effect(move || {
        let _ = query.read();
        selected.set(0);
    });

    // Build the filtered/substate-derived items list.
    let cur_state = state();
    let items_for_render: Vec<PaletteItem> = match cur_state {
        PaletteState::Browse => {
            // Filter on the FIRST whitespace token of the query (the command
            // name) rather than the whole string. Command names never contain
            // spaces, so once the user types `/<cmd> <args…>` the trailing args
            // must not break the match — e.g. `/gsd-phase 49.1 fix the search`
            // still matches `/gsd-phase`. The args are reattached on pick by
            // `ScreenChat::on_pick`.
            let q = query()
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_lowercase();
            items
                .iter()
                .filter(|p| {
                    p.cmd.to_lowercase().contains(&q) || p.label.to_lowercase().contains(&q)
                })
                .cloned()
                .collect()
        }
        PaletteState::PersonalityPick => Personality::ALL
            .iter()
            .map(|p| PaletteItem {
                section: "personality".into(),
                cmd: format!("/{}", p.label()),
                label: format!("Personality: {}", p.label()),
                kbd: vec![],
            })
            .collect(),
    };

    let len = items_for_render.len();
    let items_for_keys = items_for_render.clone();

    let on_keydown = move |e: KeyboardEvent| {
        if len == 0 {
            return;
        }
        match e.key() {
            Key::ArrowDown => {
                let s = selected();
                selected.set((s + 1) % len);
                e.prevent_default();
            }
            Key::ArrowUp => {
                let s = selected();
                selected.set((s + len - 1) % len);
                e.prevent_default();
            }
            Key::Enter => {
                if let Some(item) = items_for_keys.get(selected()) {
                    on_pick.call(item.clone());
                }
                e.prevent_default();
            }
            _ => {}
        }
    };

    // Unified list: slash commands, personalities, AND skills all render in the
    // main list (UI-SPEC §A — one flat filtered list). Skill rows are tagged
    // `◆ skill` below. Workflow rows (legacy) keep their own trailing section.
    let slash_items: Vec<PaletteItem> = items_for_render
        .iter()
        .filter(|p| p.section == "slash" || p.section == "personality" || p.section == "skill")
        .cloned()
        .collect();
    let workflow_items: Vec<PaletteItem> = items_for_render
        .iter()
        .filter(|p| p.section == "workflow")
        .cloned()
        .collect();

    let section_label = match cur_state {
        PaletteState::Browse => "commands",
        PaletteState::PersonalityPick => "personalities",
    };

    rsx! {
        div { class: "wh-pal-overlay",
            tabindex: "-1",
            onkeydown: on_keydown,
            div { class: "wh-pal",
                div { class: "wh-pal-search",
                    span { class: "wh-pal-icon", "⌘K" }
                    input {
                        placeholder: "search commands…",
                        value: "{query}",
                        oninput: move |e| query.set(e.value()),
                        autofocus: true,
                        // G-41.1-6: `autofocus` alone is unreliable for an
                        // element (re)inserted into the DOM after initial
                        // page load (Dioxus #423) — this component
                        // early-returns empty markup when closed, so the
                        // input is freshly created every time the palette
                        // opens. `onmounted` + `set_focus` is the
                        // Dioxus-documented reliable grab; kept alongside
                        // `autofocus` as a harmless fallback.
                        onmounted: move |e: Event<MountedData>| {
                            spawn(async move {
                                let _ = e.data().set_focus(true).await;
                            });
                        },
                    }
                    span { class: "wh-kbd", "esc" }
                }
                div {
                    class: "wh-pal-list",
                    role: "listbox",
                    "aria-label": "Command options",
                    div { class: "wh-pal-section", "aria-hidden": "true", "{section_label}" }
                    if slash_items.is_empty() && workflow_items.is_empty() {
                        div {
                            style: "padding: 8px 10px; color: var(--fg-dim); font-size: var(--fs-12);",
                            if !query().is_empty() { "No matching commands or skills." } else { "loading…" }
                        }
                    }
                    for (i, it) in slash_items.iter().enumerate() {
                        div {
                            key: "{it.cmd}",
                            class: "wh-pal-row",
                            class: if i == selected() { "is-active" },
                            role: "option",
                            "aria-selected": if i == selected() { "true" } else { "false" },
                            onclick: {
                                let item = it.clone();
                                move |_| on_pick.call(item.clone())
                            },
                            span { class: "wh-pal-glyph", "/" }
                            span { class: "wh-pal-cmd", "{it.cmd}" }
                            span { class: "wh-pal-desc", "— {it.label}" }
                            // Phase 41.1 Plan 06 (D-05, UI-SPEC §A): DIM `◆ skill`
                            // tag on skill rows only — distinguishes skills from
                            // commands in the unified list. `--fg-dim` (never
                            // accent) per the UI-SPEC Color contract.
                            if it.section == "skill" {
                                span {
                                    class: "wh-pal-skill-tag",
                                    style: "margin-left: 8px; color: var(--fg-dim); font-size: var(--fs-12);",
                                    "◆ skill"
                                }
                            }
                            span { class: "wh-pal-hint",
                                span { class: "wh-pal-kbd",
                                    for (j, k) in it.kbd.iter().enumerate() {
                                        span { key: "{j}", class: "wh-kbd", "{k}" }
                                    }
                                }
                            }
                        }
                    }
                    if !workflow_items.is_empty() {
                        div { class: "wh-pal-section", "Workflows" }
                        for (i, it) in workflow_items.iter().enumerate() {
                            div {
                                key: "{it.cmd}",
                                class: "wh-pal-row",
                                class: if (slash_items.len() + i) == selected() { "is-active" },
                                role: "option",
                                "aria-selected": if (slash_items.len() + i) == selected() { "true" } else { "false" },
                                onclick: {
                                    let item = it.clone();
                                    move |_| on_pick.call(item.clone())
                                },
                                span { class: "wh-pal-glyph", "▸" }
                                span { class: "wh-pal-cmd", "{it.label}" }
                                span { class: "wh-pal-desc", "{it.cmd}" }
                            }
                        }
                    }
                }
                div { class: "wh-pal-footer",
                    "↑↓ navigate · ↵ select · esc close"
                }
            }
        }
    }
}

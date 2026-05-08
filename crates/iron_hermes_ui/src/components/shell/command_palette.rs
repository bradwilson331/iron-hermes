use crate::state::{PaletteItem, PaletteState, Personality};
use dioxus::prelude::*;

/// CommandPalette — overlay with two substates per CONTEXT D-20:
///
///   - `Browse`         — filtered PALETTE_ITEMS (slash + workflow rows).
///   - `PersonalityPick` — six rows for `Personality::ALL`; selecting one
///                        writes ShellSettings.personality (Plan 04-05 wires).
///
/// Phase 4 adds (per CONTEXT D-18 + D-32 + KBD-03):
///   - Live filter: `query()` lowercase substring match against
///     `cmd` + `label` fields.
///   - Local `Signal<usize> selected` index walked by ↑/↓ keys (wraps
///     at boundaries).
///   - Enter dispatches `on_pick.call(items[selected])`.
///
/// Esc behavior is handled by the global keydown listener in WarpHermes
/// (D-17), not this component.
///
/// Per PATTERNS file risk note: `selected` resets to 0 whenever `query`
/// changes via a `use_effect` watching `query()`.
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

    // Reset selected when query changes (per PATTERNS risk note).
    use_effect(move || {
        let _ = query.read();
        selected.set(0);
    });

    // Build the filtered/substate-derived items list.
    let cur_state = state();
    let items_for_render: Vec<PaletteItem> = match cur_state {
        PaletteState::Browse => {
            let q = query().to_lowercase();
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

    let slash_items: Vec<PaletteItem> = items_for_render
        .iter()
        .filter(|p| p.section == "slash" || p.section == "personality")
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
                            style: "padding: 8px 10px; color: var(--fg-dim); font-size: 12px;",
                            if !query().is_empty() { "no results" } else { "loading…" }
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

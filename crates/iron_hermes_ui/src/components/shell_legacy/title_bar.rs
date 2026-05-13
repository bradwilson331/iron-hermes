use super::sigil::Sigil;
use crate::state::Tab;
use dioxus::prelude::*;

// Wordmark and shield assets are migrated from `src/components/hero.rs` per
// CONTEXT D-05 / UI-SPEC planner-handoff #2. They are not rendered by the
// title bar in Phase 3 (the prototype uses literal "IronHermes" text), but
// Phase 5 TweaksPanel may consume them. `#[allow(dead_code)]` silences the
// unused-const warning until then.
#[allow(dead_code)]
const WORDMARK_SVG: Asset = asset!("/assets/wordmark.svg");
#[allow(dead_code)]
const IH_SHIELD_PNG: Asset = asset!("/assets/ih-shield.png");

/// Title bar — macOS traffic-light cluster, IronHermes brand block, tab strip,
/// and `⌘K` shortcut display.
///
/// Port of `warp2ironhermes/project/app/shell.jsx` lines 62-91 per CONTEXT D-01.
/// Traffic-light hex codes (`#ff5f57`/`#febc2e`/`#28c840`) are macOS system
/// colors and are kept as literals (not tokens) per UI-SPEC line 84.
///
/// Phase 26.2 D-09: gains three EventHandler callbacks for interactive tabs
/// (click to switch, close to remove, new to create) and a `disabled` prop
/// that greys out the tab strip during streaming (D-02).
#[component]
pub fn TitleBar(
    tabs: Vec<Tab>,
    active_tab: usize,
    show_traffic_lights: bool,
    disabled: bool,
    on_tab_click: EventHandler<usize>,
    on_tab_close: EventHandler<usize>,
    on_tab_new: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "wh-titlebar",
            if show_traffic_lights {
                div {
                    class: "wh-traffic-lights",
                    "aria-hidden": "true",
                    span { style: "width: 12px; height: 12px; border-radius: 50%; background: #ff5f57;" }
                    span { style: "width: 12px; height: 12px; border-radius: 50%; background: #febc2e;" }
                    span { style: "width: 12px; height: 12px; border-radius: 50%; background: #28c840;" }
                }
            }
            div {
                class: "wh-brand-block",
                Sigil { size: 18_u16 }
                span { class: "wh-brand-name", "IronHermes" }
            }
            div {
                class: "wh-tabs",
                role: "tablist",
                "aria-label": "Sessions",
                style: if disabled { "pointer-events: none; opacity: 0.5;" } else { "" },
                for (i, t) in tabs.iter().enumerate() {
                    div {
                        key: "{i}",
                        class: "wh-tab",
                        class: if i == active_tab { "is-active" },
                        role: "tab",
                        "aria-selected": if i == active_tab { "true" } else { "false" },
                        tabindex: "0",
                        title: "{t.label}",
                        onclick: move |_| on_tab_click.call(i),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter {
                                e.prevent_default();
                                on_tab_click.call(i);
                            }
                        },
                        span {
                            class: "wh-tab-dot",
                            style: if t.live { "background: var(--success);" } else { "background: var(--fg-dim);" },
                        }
                        span { class: "wh-tab-label", "{t.label}" }
                        button {
                            class: "wh-tab-close",
                            onclick: move |evt| { evt.stop_propagation(); on_tab_close.call(i); },
                            title: "close tab",
                            "aria-label": "close tab",
                            "×"
                        }
                    }
                }
                button {
                    class: "wh-tab-new",
                    onclick: move |_| on_tab_new.call(()),
                    title: "new session",
                    "aria-label": "new session",
                    "+"
                }
            }
            div { class: "wh-titlebar-actions",
                span { style: "font-size: 11px;", "⌘K" }
            }
        }
    }
}

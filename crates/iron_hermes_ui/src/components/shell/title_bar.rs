use dioxus::prelude::*;
use crate::state::Tab;
use super::sigil::Sigil;

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
#[component]
pub fn TitleBar(tabs: Vec<Tab>, active_tab: usize, show_traffic_lights: bool) -> Element {
    rsx! {
        div { class: "wh-titlebar",
            if show_traffic_lights {
                div { style: "display: flex; gap: 8px; align-items: center; padding-right: 8px;",
                    span { style: "width: 12px; height: 12px; border-radius: 50%; background: #ff5f57;" }
                    span { style: "width: 12px; height: 12px; border-radius: 50%; background: #febc2e;" }
                    span { style: "width: 12px; height: 12px; border-radius: 50%; background: #28c840;" }
                }
            }
            div {
                style: "display: flex; align-items: center; gap: 8px; padding-right: 12px; border-right: 1px solid var(--w-border); height: 100%;",
                Sigil { size: 18_u16 }
                span {
                    style: "color: var(--accent-primary); font-weight: 700; font-size: 12px;",
                    "IronHermes"
                }
            }
            div { class: "wh-tabs",
                for (i, t) in tabs.iter().enumerate() {
                    div {
                        key: "{i}",
                        class: "wh-tab",
                        class: if i == active_tab { "is-active" },
                        span {
                            class: "wh-tab-dot",
                            style: if t.live { "background: var(--success);" } else { "background: var(--fg-dim);" },
                        }
                        "{t.label}"
                        span {
                            style: "color: var(--fg-disabled); margin-left: 4px; font-size: 11px;",
                            "×"
                        }
                    }
                }
                button {
                    style: "padding: 0 10px; color: var(--fg-dim); font-size: 14px; font-weight: 700; background: none; border: none; cursor: default;",
                    "+"
                }
            }
            div { class: "wh-titlebar-actions",
                span { style: "font-size: 11px;", "⌘K" }
            }
        }
    }
}

//! Phase 26.2.1 — TweaksPanel: gear FAB + slide-up panel.
//!
//! Mirrors `app.html`'s `.tweaks-fab` button + `.tweaks` panel (lines
//! 1480–1521). The panel is ALWAYS mounted; the `is-open` class drives the
//! CSS slide-up animation (defined in the bundle's `components.css`) — this
//! matches the prototype pattern and avoids signal teardown on close.
//!
//! Controls write back through two context signals:
//!   - `Signal<UiPrefs>` — accent, footer, density (each `with_mut` write)
//!   - `ThemeContext.0`  — the active theme slug (`Signal<String>`)
//!
//! Phase 54.4 Plan 02 (D-05/D-07/D-26): the wheel-size slider, Breadcrumb
//! toggle and Chat rail toggle are retired — the docked header wheel is pinned
//! by `--ih-wheel-size`, the status strip is permanent and `WheelRail` is gone.
//!
//! Persistence is owned by Plan 03's hydration-gated effects — TweaksPanel
//! does NOT touch localStorage directly.

use crate::state::ThemeContext;
use crate::ui_prefs::{AccentColor, Density, UiPrefs};
use dioxus::prelude::*;

/// Gear FAB at bottom-right + the slide-up tweaks panel.
///
/// The two siblings are emitted from one `rsx!` tree. Visibility is driven
/// by the `is-open` class on the panel — both elements are always mounted
/// so signal subscriptions persist across open/close cycles.
#[component]
pub fn TweaksPanel() -> Element {
    let mut prefs = use_context::<Signal<UiPrefs>>();
    let mut theme = use_context::<ThemeContext>().0;

    // Local open/closed flag — not in shared context (D-26 / D-16).
    let mut open = use_signal(|| false);

    // ----- Theme list (D-14: 5 themes) -----
    let theme_names: [&'static str; 5] = [
        "slate-dark",
        "slate-light",
        "iron-dark",
        "terminal-dark",
        "parchment-light",
    ];

    // ----- Accent list (D-13: 5 accents) -----
    let accent_list: [(AccentColor, &'static str); 5] = [
        (AccentColor::Teal, "TEAL"),
        (AccentColor::Orange, "ORANGE"),
        (AccentColor::Green, "GREEN"),
        (AccentColor::Violet, "VIOLET"),
        (AccentColor::Amber, "AMBER"),
    ];

    // Snapshot the current theme + accent for class-conditional rendering.
    // Done at the top of the closure tree so the borrows drop before the
    // RSX builder runs (signal-borrow safety per clippy.toml).
    let current_theme = theme.read().clone();
    let current_accent = prefs.read().accent;
    let current_footer = prefs.read().footer;
    let current_density = prefs.read().density;
    let is_open = *open.read();

    rsx! {
        // Gear FAB — always rendered, toggles `open` flag.
        button {
            class: "tweaks-fab",
            "aria-label": "Open tweaks panel",
            title: "Tweaks",
            onclick: move |_| {
                let cur = *open.read();
                open.set(!cur);
            },
            "⚙"
        }

        // Slide-up panel — always rendered; `is-open` class drives CSS.
        aside {
            class: "tweaks",
            class: if is_open { "is-open" },
            role: "dialog",
            "aria-label": "UI tweaks",

            div { class: "tweaks-head",
                span { class: "tweaks-title", "── Tweaks" }
                button {
                    class: "tweaks-x",
                    "aria-label": "Close",
                    onclick: move |_| open.set(false),
                    "×"
                }
            }

            div { class: "tweaks-body",

                // ---- Theme picker (5 themes) ----
                div { class: "tweaks-section",
                    div { class: "tweaks-label", "Theme" }
                    div { class: "tweaks-row",
                        for theme_name in theme_names {
                            button {
                                key: "{theme_name}",
                                class: "tweaks-opt",
                                class: if current_theme == theme_name { "is-active" },
                                onclick: move |_| theme.set(theme_name.to_string()),
                                "{theme_name}"
                            }
                        }
                    }
                }

                // ---- Accent picker (5 colors) ----
                div { class: "tweaks-section",
                    div { class: "tweaks-label", "Accent" }
                    div { class: "tweaks-row",
                        for (variant, label) in accent_list {
                            button {
                                key: "{label}",
                                class: "tweaks-opt",
                                class: if current_accent == variant { "is-active" },
                                onclick: move |_| {
                                    prefs.with_mut(|p| p.accent = variant);
                                },
                                "{label}"
                            }
                        }
                    }
                }

                // ---- Density (Comfy / Dense) ----
                div { class: "tweaks-section",
                    div { class: "tweaks-label", "Density" }
                    div { class: "tweaks-row",
                        button {
                            class: "tweaks-opt",
                            class: if current_density == Density::Comfy { "is-active" },
                            onclick: move |_| {
                                prefs.with_mut(|p| p.density = Density::Comfy);
                            },
                            "Comfy"
                        }
                        button {
                            class: "tweaks-opt",
                            class: if current_density == Density::Dense { "is-active" },
                            onclick: move |_| {
                                prefs.with_mut(|p| p.density = Density::Dense);
                            },
                            "Dense"
                        }
                    }
                }

                // ---- Chrome toggles (footer ticker only) ----
                div { class: "tweaks-section",
                    div { class: "tweaks-label", "Chrome" }
                    div { class: "tweaks-row",
                        span { class: "lbl", "Footer ticker" }
                        button {
                            class: "tweaks-toggle",
                            class: if current_footer { "is-on" },
                            onclick: move |_| {
                                prefs.with_mut(|p| p.footer = !p.footer);
                            },
                            if current_footer { "ON" } else { "OFF" }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Phase 54.4 Plan 02 (D-05/D-07/D-26): the wheel-size slider, Chat rail
    //! and Breadcrumb controls are retired because the docked header makes
    //! them dead. Source-text assertions; needles are assembled from fragments
    //! so the test cannot match its own source.

    const TWEAKS_PANEL_RS: &str = include_str!("tweaks_panel.rs");

    #[test]
    fn tweaks_panel_has_no_retired_chrome_controls() {
        for field in ["rail", "breadcrumb", "wheel_size"] {
            let needle = ["p.", field].concat();
            assert!(
                !TWEAKS_PANEL_RS.contains(&needle),
                "tweaks_panel.rs still references retired control accessor `{needle}`"
            );
        }
        let kept = ["p.", "footer"].concat();
        assert!(
            TWEAKS_PANEL_RS.contains(&kept),
            "tweaks_panel.rs must keep the footer ticker control (`{kept}`)"
        );
    }
}

//! Phase 26.2.1 — TweaksPanel: gear FAB + slide-up panel.
//!
//! Mirrors `app.html`'s `.tweaks-fab` button + `.tweaks` panel (lines
//! 1480–1521). The panel is ALWAYS mounted; the `is-open` class drives the
//! CSS slide-up animation (defined in the bundle's `components.css`) — this
//! matches the prototype pattern and avoids signal teardown on close.
//!
//! Controls write back through three context signals:
//!   - `Signal<UiPrefs>` — accent, wheel_size, breadcrumb,
//!     footer, density, rail (each `with_mut` write)
//!   - `ThemeContext.0`  — the active theme slug (`Signal<String>`)
//!   - `Signal<WheelState>` — wheel_size slider mirrors UiPrefs.wheel_size
//!     onto WheelState.size (must_have #5)
//!
//! Persistence is owned by Plan 03's hydration-gated effects — TweaksPanel
//! does NOT touch localStorage directly.

use crate::state::{ThemeContext, WheelState};
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
    let mut wheel_state = use_context::<Signal<WheelState>>();

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
    let current_wheel_size = prefs.read().wheel_size;
    let current_breadcrumb = prefs.read().breadcrumb;
    let current_footer = prefs.read().footer;
    let current_rail = prefs.read().rail;
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

                // ---- Wheel size slider (240..=640 in steps of 20) ----
                div { class: "tweaks-section",
                    div { class: "tweaks-label",
                        "Wheel size "
                        span { class: "val", "{current_wheel_size}" }
                    }
                    div { class: "tweaks-row",
                        input {
                            r#type: "range",
                            min: "240",
                            max: "640",
                            step: "20",
                            value: "{current_wheel_size}",
                            oninput: move |evt| {
                                let val: f64 = evt.value().parse().unwrap_or(240.0);
                                prefs.with_mut(|p| p.wheel_size = val);
                                wheel_state.with_mut(|ws| ws.size = val);
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

                // ---- Chrome toggles (breadcrumb / footer / rail) ----
                div { class: "tweaks-section",
                    div { class: "tweaks-label", "Chrome" }
                    div { class: "tweaks-row",
                        span { class: "lbl", "Breadcrumb" }
                        button {
                            class: "tweaks-toggle",
                            class: if current_breadcrumb { "is-on" },
                            onclick: move |_| {
                                prefs.with_mut(|p| p.breadcrumb = !p.breadcrumb);
                            },
                            if current_breadcrumb { "ON" } else { "OFF" }
                        }
                    }
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
                    div { class: "tweaks-row",
                        span { class: "lbl", "Chat rail" }
                        button {
                            class: "tweaks-toggle",
                            class: if current_rail { "is-on" },
                            onclick: move |_| {
                                prefs.with_mut(|p| p.rail = !p.rail);
                            },
                            if current_rail { "ON" } else { "OFF" }
                        }
                    }
                }
            }
        }
    }
}

//! Phase 26.2.1 Plan 07 — Settings screen (LIVE-wired, read-only Runtime).
//!
//! Replaces the Plan 03 placeholder with two logical blocks:
//!
//! 1. **Runtime block (read-only)** — renders `ConfigSummary { model,
//!    provider, context_length, memory_enabled }` from the existing
//!    `get_config_summary()` server fn. Per D-03 all `config.yaml`-bound
//!    fields render visually disabled with a `(coming soon)` affordance —
//!    no write-back exists yet (deferred to 26.2.12).
//!
//! 2. **UI Preferences block (writable)** — theme picker (5 themes),
//!    accent picker (5 `AccentColor` variants), wheel-size slider
//!    (240..=640 mirroring Plan 04 `MIN_SIZE` / `MAX_SIZE`), four boolean
//!    toggles (breadcrumb / footer / rail), and a comfy/dense
//!    density two-state. All writes target `Signal<UiPrefs>` plus the
//!    theme `Signal<String>` (consumed via the B-03 `ThemeContext`
//!    newtype) plus `Signal<WheelState>` for the wheel-size mirror.
//!    Plan 05's `theme_effects.rs` performs the actual DOM mutation
//!    when the theme name changes.
//!
//! Plus a small "Other Screens" reachability nav for Soul / Schedules /
//! Office (CONTEXT D-10 Claude's Discretion — those three screens exist in
//! `app.html` but are NOT on the wheel).
//!
//! Per D-02 the server tree is byte-for-byte untouched.
//! Per D-24 the theme picker is the only Settings write-back that drives a
//! DOM side-effect — it writes the `Signal<String>` only; `ThemeEffects`
//! in Plan 05 performs the `<html data-theme=…>` mutation.

use crate::ui_prefs::{AccentColor, Density, UiPrefs};
use dioxus::prelude::*;

/// Settings screen — `<section id="screen-settings">` ported from
/// `app.html` line 1356. Two blocks: Runtime (read-only ConfigSummary)
/// and UI Preferences (writable UiPrefs + theme + wheel-size mirror).
#[component]
pub fn ScreenSettings(is_active: bool) -> Element {
    // Context — UiPrefs, theme (newtype-disambiguated per B-03), and the
    // wheel-state mirror (for the dual-write that Plan 05's TweaksPanel
    // also performs on size changes).
    let mut prefs = use_context::<Signal<UiPrefs>>();
    let mut theme = use_context::<crate::state::ThemeContext>().0;
    let mut wheel_state = use_context::<Signal<crate::state::WheelState>>();

    // Read-only runtime block — fetch ConfigSummary on mount.
    //
    // Note: `ConfigSummary` field names in `src/server/api.rs` are `model`
    // and `provider` (not `model_name` / `provider_name` as the Plan 07
    // spec references). D-02 makes api.rs the source of truth, so this
    // file matches the real field names.
    let summary_resource = use_server_future(crate::server::api::get_config_summary)?;
    let (model_name, provider_name, context_length, memory_enabled) =
        match summary_resource() {
            Some(Ok(c)) => (c.model, c.provider, c.context_length, c.memory_enabled),
            _ => ("loading…".to_string(), "…".to_string(), 0_u32, false),
        };

    // Drop all signal borrows into Copy/Clone locals before constructing
    // event closures (clippy.toml signal-borrow safety — never hold a
    // read borrow across a `.set()` or `.with_mut()` in the same closure).
    let (
        cur_theme,
        cur_accent,
        cur_wheel_size,
        cur_breadcrumb,
        cur_footer,
        cur_rail,
        cur_density,
    ) = {
        let p = prefs.read();
        let t = theme.read();
        (
            t.clone(),
            p.accent,
            p.wheel_size,
            p.breadcrumb,
            p.footer,
            p.rail,
            p.density,
        )
    };

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-settings",
            "data-screen-label": "12 Settings",

            // ── Screen header ────────────────────────────────────────
            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 12" }
                    h1 { class: "screen-title", "Settings" }
                    p { class: "screen-sub",
                        "Runtime configuration is read-only this phase (D-03). UI preferences persist to localStorage."
                    }
                }
            }

            // ── Block 1: Runtime (read-only ConfigSummary) ───────────
            div { class: "panel",
                div { class: "panel-title",
                    "Runtime "
                    span { style: "color: var(--gray); font-weight: 400; letter-spacing: 0.04em; font-size: 10px;",
                        "// read-only — coming soon"
                    }
                }

                div { class: "field-row is-disabled", style: "opacity: 0.6;",
                    div { class: "field-label",
                        "Model"
                        span { class: "help", "(coming soon) — set via config.yaml" }
                    }
                    input {
                        class: "field-input",
                        readonly: "true",
                        disabled: "true",
                        value: "{model_name}",
                    }
                }
                div { class: "field-row is-disabled", style: "opacity: 0.6;",
                    div { class: "field-label",
                        "Provider"
                        span { class: "help", "(coming soon) — set via config.yaml" }
                    }
                    input {
                        class: "field-input",
                        readonly: "true",
                        disabled: "true",
                        value: "{provider_name}",
                    }
                }
                div { class: "field-row is-disabled", style: "opacity: 0.6;",
                    div { class: "field-label",
                        "Context length"
                        span { class: "help", "(coming soon) — resolved per model" }
                    }
                    input {
                        class: "field-input",
                        readonly: "true",
                        disabled: "true",
                        value: "{context_length}",
                    }
                }
                div { class: "field-row is-disabled", style: "opacity: 0.6;",
                    div { class: "field-label",
                        "Memory"
                        span { class: "help", "(coming soon) — set via config.yaml" }
                    }
                    input {
                        class: "field-input",
                        readonly: "true",
                        disabled: "true",
                        value: if memory_enabled { "ENABLED" } else { "DISABLED" },
                    }
                }
            }

            // ── Block 2: UI Preferences (writable) ───────────────────
            div { class: "panel",
                div { class: "panel-title", "UI Preferences" }

                // ── Theme picker (5 options) ─────────────────────────
                div { class: "field-row",
                    div { class: "field-label",
                        "Theme"
                        span { class: "help", "swaps the [data-theme] attribute on <html>" }
                    }
                    div { style: "display: flex; flex-wrap: wrap; gap: 6px;",
                        for name in [
                            "slate-dark",
                            "slate-light",
                            "iron-dark",
                            "terminal-dark",
                            "parchment-light",
                        ] {
                            button {
                                key: "{name}",
                                class: "btn btn--sm",
                                class: if cur_theme == name { "is-active" },
                                onclick: move |_| theme.set(name.to_string()),
                                "{name}"
                            }
                        }
                    }
                }

                // ── Accent picker (5 AccentColor variants) ───────────
                div { class: "field-row",
                    div { class: "field-label",
                        "Accent"
                        span { class: "help", "primary highlight color" }
                    }
                    div { style: "display: flex; flex-wrap: wrap; gap: 6px;",
                        for (variant, label) in [
                            (AccentColor::Teal, "TEAL"),
                            (AccentColor::Orange, "ORANGE"),
                            (AccentColor::Green, "GREEN"),
                            (AccentColor::Violet, "VIOLET"),
                            (AccentColor::Amber, "AMBER"),
                        ] {
                            button {
                                key: "{label}",
                                class: "btn btn--sm",
                                class: if cur_accent == variant { "is-active" },
                                onclick: move |_| prefs.with_mut(|p| p.accent = variant),
                                "{label}"
                            }
                        }
                    }
                }

                // ── Wheel size slider (240..=640) ────────────────────
                div { class: "field-row",
                    div { class: "field-label",
                        "Wheel size"
                        span { class: "help", "mirrors UiPrefs.wheel_size onto WheelState.size" }
                    }
                    input {
                        r#type: "range",
                        min: "240",
                        max: "640",
                        step: "20",
                        value: "{cur_wheel_size}",
                        oninput: move |evt| {
                            let val: f64 = evt.value().parse().unwrap_or(240.0);
                            prefs.with_mut(|p| p.wheel_size = val);
                            wheel_state.with_mut(|ws| ws.size = val);
                        },
                    }
                    span { class: "row-sub", "{cur_wheel_size as u32}px" }
                }

                // ── Boolean toggle: Breadcrumb ───────────────────────
                div { class: "field-row",
                    div { class: "field-label",
                        "Breadcrumb"
                        span { class: "help", "NODE HERMES-7 › BRIDGE › … chip" }
                    }
                    button {
                        class: "btn btn--sm",
                        class: if cur_breadcrumb { "is-active" },
                        onclick: move |_| prefs.with_mut(|p| p.breadcrumb = !p.breadcrumb),
                        if cur_breadcrumb { "ON" } else { "OFF" }
                    }
                }

                // ── Boolean toggle: Footer ───────────────────────────
                div { class: "field-row",
                    div { class: "field-label",
                        "Footer"
                        span { class: "help", "app-footer strip" }
                    }
                    button {
                        class: "btn btn--sm",
                        class: if cur_footer { "is-active" },
                        onclick: move |_| prefs.with_mut(|p| p.footer = !p.footer),
                        if cur_footer { "ON" } else { "OFF" }
                    }
                }

                // ── Boolean toggle: Rail ─────────────────────────────
                div { class: "field-row",
                    div { class: "field-label",
                        "Rail"
                        span { class: "help", "vertical rail on the chat screen" }
                    }
                    button {
                        class: "btn btn--sm",
                        class: if cur_rail { "is-active" },
                        onclick: move |_| prefs.with_mut(|p| p.rail = !p.rail),
                        if cur_rail { "ON" } else { "OFF" }
                    }
                }

                // ── Density two-state ────────────────────────────────
                div { class: "field-row",
                    div { class: "field-label",
                        "Density"
                        span { class: "help", "per-row vertical density" }
                    }
                    div { style: "display: flex; gap: 6px;",
                        button {
                            class: "btn btn--sm",
                            class: if cur_density == Density::Comfy { "is-active" },
                            onclick: move |_| prefs.with_mut(|p| p.density = Density::Comfy),
                            "COMFY"
                        }
                        button {
                            class: "btn btn--sm",
                            class: if cur_density == Density::Dense { "is-active" },
                            onclick: move |_| prefs.with_mut(|p| p.density = Density::Dense),
                            "DENSE"
                        }
                    }
                }
            }

            // ── Block 3: Other Screens (Soul/Schedules/Office) ───────
            // CONTEXT D-10 Claude's Discretion: these three screens exist
            // in app.html but are NOT on the wheel; Settings is their
            // reachability path.
            div { class: "panel",
                div { class: "panel-title", "Other Screens" }
                div { class: "field-row",
                    div { class: "field-label",
                        "Reach"
                        span { class: "help", "screens not on the wheel" }
                    }
                    div { style: "display: flex; gap: 6px;",
                        SettingsScreenLink { target: crate::state::Screen::Soul, label: "SOUL" }
                        SettingsScreenLink { target: crate::state::Screen::Schedules, label: "SCHEDULES" }
                        SettingsScreenLink { target: crate::state::Screen::Office, label: "OFFICE" }
                    }
                }
            }
        }
    }
}

/// Sub-component for the "Other Screens" nav — one button per reachable
/// off-wheel screen (CONTEXT D-10 Claude's Discretion).
#[component]
fn SettingsScreenLink(target: crate::state::Screen, label: &'static str) -> Element {
    let mut active_screen = use_context::<Signal<crate::state::Screen>>();
    let target_copy = target;
    rsx! {
        button {
            class: "btn btn--sm",
            onclick: move |_| active_screen.set(target_copy),
            "{label}"
        }
    }
}

//! Phase 26.2.1 Plan 07 — Settings screen (LIVE-wired, read-only Runtime).
//!
//! Phase 54.4 Plan 16 (G-54.4-25): the screen opens on a General | Orb tab
//! strip. General holds every block below, unchanged. Orb holds the
//! standalone orb appearance panel (`voice_settings::OrbSettingsPanel`), so
//! the orb colour controls no longer depend on the Voice form's "Applies to"
//! target. Since G-54.4-34 the General tab's Voice panel no longer shows the
//! orb Appearance section; in System the Orb tab is its only home.
//!
//! Replaces the Plan 03 placeholder with several logical blocks:
//!
//! 1. **Runtime block (read-only)** — renders `ConfigSummary { model,
//!    provider, context_length, memory_enabled }` from the existing
//!    `get_config_summary()` server fn. Per D-03 all `config.yaml`-bound
//!    fields render visually disabled with a `(coming soon)` affordance —
//!    no write-back exists yet (deferred to 26.2.12).
//!
//! 1.4 **Agent block (writable, Phase 46.9 Plan 05 D-01)** — the editable
//!    `config.agent` subset (max_iterations, context_compression,
//!    tool_delay_secs, system_message, context_engine, compression_threshold,
//!    clarify_timeout_secs, timezone) via `get_agent_config`/
//!    `update_agent_config`. Always renders resolved values (serde defaults
//!    for absent keys); a distinct ghost `.field-row` state while loading.
//!
//! 1.5 **Voice / STT / TTS / VAD block (writable)** — Phase 36.17.10.
//!
//! 1.6 **Kanban block (writable, Phase 46.9 Plan 05 D-02)** — brand-new
//!    embedded panel (`kanban_config::KanbanConfigPanel`) for
//!    `ironhermes_kanban::KanbanConfig`, round-tripped through the untyped
//!    `config.kanban: serde_yaml::Value` (Pitfall 5). Completes D-02's six
//!    in-scope config surfaces for this phase.
//!
//! 2. **UI Preferences block (writable)** — theme picker (5 themes),
//!    accent picker (5 `AccentColor` variants), a footer boolean toggle,
//!    and a comfy/dense density two-state. All writes target
//!    `Signal<UiPrefs>` plus the theme `Signal<String>` (consumed via the
//!    B-03 `ThemeContext` newtype). Plan 05's `theme_effects.rs` performs
//!    the actual DOM mutation when the theme name changes. Phase 54.4
//!    Plan 02 (D-05/D-07/D-26) retired the wheel-size slider and the
//!    breadcrumb / rail toggles: the docked header wheel is pinned by
//!    `--ih-wheel-size` and the status strip is permanent.
//!
//! Plus a small "Other Screens" reachability nav for Soul / Schedules /
//! Office (CONTEXT D-10 Claude's Discretion — those three screens exist in
//! `app.html` but are NOT on the wheel).
//!
//! Per D-02 the server tree is byte-for-byte untouched.
//! Per D-24 the theme picker is the only Settings write-back that drives a
//! DOM side-effect — it writes the `Signal<String>` only; `ThemeEffects`
//! in Plan 05 performs the `<html data-theme=…>` mutation.

use crate::server::api::{get_agent_config, update_agent_config, AgentWritePayload};
use crate::ui_prefs::{AccentColor, Density, UiPrefs};
use dioxus::prelude::*;

/// Settings screen — `<section id="screen-settings">` ported from
/// `app.html` line 1356. Two blocks: Runtime (read-only ConfigSummary)
/// and UI Preferences (writable UiPrefs + theme + wheel-size mirror).
#[component]
pub fn ScreenSettings(is_active: bool) -> Element {
    // Context — UiPrefs and theme (newtype-disambiguated per B-03).
    let mut prefs = use_context::<Signal<UiPrefs>>();
    let mut theme = use_context::<crate::state::ThemeContext>().0;

    // Selected tab (G-54.4-25). Declared BEFORE the summary server future below:
    // that future early-returns with `?` while it loads, and any hook declared
    // after the early return is skipped on those renders, so a later hook would
    // change order once the summary arrives. The tab is not persisted: the
    // screen opens on General every visit.
    let tab = use_signal(|| SettingsTab::General);
    // Bumped by the "set via config.yaml" hint links; the config-file panel
    // subscribes. Same ordering rule as `tab`: above the `?` early return below.
    let mut focus_tick = use_signal(|| 0u32);

    // Read-only runtime block — fetch ConfigSummary on mount.
    //
    // Note: `ConfigSummary` field names in `src/server/api.rs` are `model`
    // and `provider` (not `model_name` / `provider_name` as the Plan 07
    // spec references). D-02 makes api.rs the source of truth, so this
    // file matches the real field names.
    let summary_resource = use_server_future(crate::server::api::get_config_summary)?;
    let (model_name, provider_name, context_length, context_length_source, memory_enabled) =
        match summary_resource() {
            Some(Ok(c)) => (
                c.model,
                c.provider,
                Some(c.context_length),
                c.context_length_source,
                c.memory_enabled,
            ),
            _ => (
                "loading…".to_string(),
                "…".to_string(),
                None,
                crate::server::api::ContextWindowSource::default(),
                false,
            ),
        };
    // D-13: effective window plus the word naming which tier produced it.
    let context_length_display = crate::components::hermes_app::screens::models::window_placeholder(
        context_length,
        context_length_source,
    );

    // Drop all signal borrows into Copy/Clone locals before constructing
    // event closures (clippy.toml signal-borrow safety — never hold a
    // read borrow across a `.set()` or `.with_mut()` in the same closure).
    let (cur_theme, cur_accent, cur_footer, cur_density) = {
        let p = prefs.read();
        let t = theme.read();
        (t.clone(), p.accent, p.footer, p.density)
    };
    let cur_tab = *tab.read();

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
                }
            }

            // ── Tab strip (Phase 54.4 Plan 16, G-54.4-25) ────────────
            SettingsTabs { active: tab }

            if cur_tab == SettingsTab::General {
                // General: every existing block, unchanged. `gap: inherit` keeps
                // `.screen`'s 24px (16px dense) spacing between the panels.
                div {
                    id: SettingsTab::General.panel_id(),
                    role: "tabpanel",
                    "aria-labelledby": SettingsTab::General.tab_id(),
                    style: "display:flex; flex-direction:column; gap:inherit;",

                    // ── Block 1: Runtime (model/provider — managed in config.yaml) ───────
                    div { class: "panel",
                        div { class: "panel-title",
                            "Runtime "
                            span { style: "color: var(--gray); font-weight: 400; letter-spacing: 0.04em; font-size: 10px;",
                                "// managed in config.yaml"
                            }
                        }

                        div { class: "field-row is-disabled", style: "opacity: 0.6;",
                            div { class: "field-label",
                                "Model"
                                button {
                                    r#type: "button",
                                    class: "help cfg-hintlink",
                                    onclick: move |_| focus_tick += 1,
                                    "set via config.yaml"
                                }
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
                                button {
                                    r#type: "button",
                                    class: "help cfg-hintlink",
                                    onclick: move |_| focus_tick += 1,
                                    "set via config.yaml"
                                }
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
                                span { class: "help", "resolved per model" }
                            }
                            input {
                                class: "field-input",
                                readonly: "true",
                                disabled: "true",
                                value: "{context_length_display}",
                            }
                        }
                        div { class: "field-row is-disabled", style: "opacity: 0.6;",
                            div { class: "field-label",
                                "Memory"
                                button {
                                    r#type: "button",
                                    class: "help cfg-hintlink",
                                    onclick: move |_| focus_tick += 1,
                                    "set via config.yaml"
                                }
                            }
                            input {
                                class: "field-input",
                                readonly: "true",
                                disabled: "true",
                                value: if memory_enabled { "ENABLED" } else { "DISABLED" },
                            }
                        }
                    }

                    // ── Block 1.4: Agent (writable → config.agent) ────────────────────────
                    // Phase 46.9 Plan 05 (D-01): editable subset of config.agent, wrapped as
                    // a titled panel block (matching Runtime / Voice / UI Preferences).
                    div { class: "panel",
                        div { class: "panel-title", "Agent" }
                        AgentConfigPanel {}
                    }

                    // ── Block 1.45: Login theme (write-gated → web_ui.auth.login_theme) ──
                    // Phase 47.3 Plan 06 (D-01/D-03): the pre-auth login page's visual
                    // treatment picker. Honors the SAME security.web_config_write_enabled
                    // gate as the Agent panel above — D-03's hard requirement is that the
                    // disabled state is genuinely inert (true `disabled` attribute on every
                    // input), never a control that looks interactive and silently no-ops.
                    div { class: "panel",
                        div { class: "panel-title", "Login Theme" }
                        LoginThemePanel {}
                    }

                    // ── Block 1.5: Voice / STT / TTS / VAD (writable → config.yaml) ──────
                    // Phase 36.17.10: two-way config write-back form, wrapped as a titled
                    // panel block (matching Runtime / UI Preferences) and positioned between
                    // them. on_close is a no-op (this is a screen, not a dismissable overlay).
                    // VoiceStatusState context is provided by HermesApp (mod.rs:535).
                    // The orb appearance controls live only on the Orb tab (G-54.4-34),
                    // so this embed opts out of them; the voice-mode overlay keeps them.
                    div { class: "panel voice-settings-embed",
                        div { class: "panel-title", "Voice" }
                        crate::components::hermes_app::voice_settings::VoiceSettings { on_close: move |_| {}, show_orb_appearance: false }
                    }

                    // ── Block 1.6: Kanban (writable → config.kanban, brand-new surface) ──
                    // Phase 46.9 Plan 05 (D-02): embedded panel — NOT a new top-level
                    // screen/wheel entry — positioned directly below the voice-settings-embed
                    // panel per the same embed pattern.
                    div { class: "panel",
                        div { class: "panel-title", "Kanban" }
                        crate::components::hermes_app::kanban_config::KanbanConfigPanel {}
                    }

                    // ── Block 1.7: Config file (YAML) — Phase 54.13 (D-10) ──
                    // The panel renders its own `.panel` wrapper (id `settings-config-file`).
                    crate::components::hermes_app::screens::config_yaml_panel::ConfigYamlPanel { focus_tick }

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
            } else if cur_tab == SettingsTab::Orb {
                // Orb: the standalone orb appearance panel (G-54.4-25).
                div {
                    id: SettingsTab::Orb.panel_id(),
                    role: "tabpanel",
                    "aria-labelledby": SettingsTab::Orb.tab_id(),
                    style: "display:flex; flex-direction:column; gap:inherit;",
                    div { class: "panel voice-settings-embed",
                        div { class: "panel-title", "Orb appearance" }
                        crate::components::hermes_app::voice_settings::OrbSettingsPanel {}
                    }
                }
            } else {
                // Phase 54.9 Plan 04 (D-01): the menu layout editor.
                div {
                    id: SettingsTab::Menu.panel_id(),
                    role: "tabpanel",
                    "aria-labelledby": SettingsTab::Menu.tab_id(),
                    "aria-label": "Menu layout",
                    style: "display:flex; flex-direction:column; gap:inherit;",
                    super::settings_menu::MenuSettingsPanel {}
                }
            }
        }
    }
}

/// Phase 54.4 Plan 16 (G-54.4-25) + Phase 54.9 (D-01): the three tabs of the System screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsTab {
    General,
    Orb,
    Menu,
}

impl SettingsTab {
    pub(crate) fn label(self) -> &'static str {
        match self {
            SettingsTab::General => "General",
            SettingsTab::Orb => "Orb",
            SettingsTab::Menu => "Menu",
        }
    }

    pub(crate) fn tab_id(self) -> &'static str {
        match self {
            SettingsTab::General => "ih-settings-tab-general",
            SettingsTab::Orb => "ih-settings-tab-orb",
            SettingsTab::Menu => "ih-settings-tab-menu",
        }
    }

    pub(crate) fn panel_id(self) -> &'static str {
        match self {
            SettingsTab::General => "ih-settings-panel-general",
            SettingsTab::Orb => "ih-settings-panel-orb",
            SettingsTab::Menu => "ih-settings-panel-menu",
        }
    }

    /// The tab to the right, wrapping from the last to the first.
    pub(crate) fn next(self) -> SettingsTab {
        match self {
            SettingsTab::General => SettingsTab::Orb,
            SettingsTab::Orb => SettingsTab::Menu,
            SettingsTab::Menu => SettingsTab::General,
        }
    }

    /// The tab to the left, wrapping from the first to the last.
    pub(crate) fn prev(self) -> SettingsTab {
        match self {
            SettingsTab::General => SettingsTab::Menu,
            SettingsTab::Orb => SettingsTab::General,
            SettingsTab::Menu => SettingsTab::Orb,
        }
    }
}

/// Tab order, left to right.
const SETTINGS_TABS: [SettingsTab; 3] = [SettingsTab::General, SettingsTab::Orb, SettingsTab::Menu];

/// The General | Orb | Menu tab strip, following the Logs screen's `.tabs` /
/// `role="tablist"` precedent and Kanban's Left/Right keyboard precedent.
/// Every tab stays in the tab order (no roving tabindex), so arrow selection
/// never strands focus.
#[component]
fn SettingsTabs(active: Signal<SettingsTab>) -> Element {
    let mut active = active;
    let cur = *active.read();

    rsx! {
        div {
            class: "tabs",
            role: "tablist",
            "aria-label": "Settings sections",
            for tab in SETTINGS_TABS {
                button {
                    key: "{tab.tab_id()}",
                    id: tab.tab_id(),
                    class: "tab",
                    class: if cur == tab { "is-active" },
                    r#type: "button",
                    role: "tab",
                    "aria-selected": if cur == tab { "true" } else { "false" },
                    onclick: move |_| active.set(tab),
                    onkeydown: move |evt: KeyboardEvent| {
                        let target = match evt.key() {
                            Key::ArrowRight => Some(tab.next()),
                            Key::ArrowLeft => Some(tab.prev()),
                            _ => None,
                        };
                        if let Some(next) = target {
                            evt.prevent_default();
                            active.set(next);
                            crate::components::hermes_app::header::focus_element_by_id(next.tab_id());
                        }
                    },
                    "{tab.label()}"
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

/// Phase 46.9 Plan 05 (D-01): editable `config.agent` panel. Modeled on
/// `kanban_config::KanbanConfigPanel` — its own `use_resource` + `loaded`
/// latch (not a `use_server_future(...)?` bubbled to the whole screen) so
/// this block gets a distinct ghost `.field-row` loading state independent
/// of the Runtime block above it and the Voice/Kanban panels below it.
#[component]
fn AgentConfigPanel() -> Element {
    let mut max_iterations_sig = use_signal(|| 50u64);
    let mut context_compression_sig = use_signal(|| "0.5".to_string());
    let mut tool_delay_secs_sig = use_signal(|| "1.0".to_string());
    let mut system_message_sig = use_signal(String::new);
    let mut context_engine_sig = use_signal(|| "summarizing".to_string());
    let mut compression_threshold_sig = use_signal(|| "0.5".to_string());
    let mut clarify_timeout_secs_sig = use_signal(|| 120u64);
    let mut timezone_sig = use_signal(String::new);

    let mut loaded = use_signal(|| false);
    let mut write_enabled = use_signal(|| false);
    let config_resource = use_resource(move || async move { get_agent_config().await });

    if !*loaded.read() {
        if let Some(Ok(snapshot)) = config_resource.read().as_ref() {
            max_iterations_sig.set(snapshot.max_iterations as u64);
            context_compression_sig.set(snapshot.context_compression.to_string());
            tool_delay_secs_sig.set(snapshot.tool_delay_secs.to_string());
            system_message_sig.set(snapshot.system_message.clone());
            context_engine_sig.set(snapshot.context_engine.clone());
            compression_threshold_sig.set(snapshot.compression_threshold.to_string());
            clarify_timeout_secs_sig.set(snapshot.clarify_timeout_secs);
            timezone_sig.set(snapshot.timezone.clone().unwrap_or_default());
            write_enabled.set(snapshot.web_config_write_enabled);
            loaded.set(true);
        }
    }

    let is_loading = config_resource.read().is_none();
    let load_failed = matches!(config_resource.read().as_ref(), Some(Err(_)));

    let mut saving = use_signal(|| false);
    let mut save_error: Signal<Option<String>> = use_signal(|| None);
    let mut save_ok = use_signal(|| false);

    let max_iterations_val = *max_iterations_sig.read();
    let context_compression_val = context_compression_sig.read().clone();
    let tool_delay_secs_val = tool_delay_secs_sig.read().clone();
    let system_message_val = system_message_sig.read().clone();
    let context_engine_val = context_engine_sig.read().clone();
    let compression_threshold_val = compression_threshold_sig.read().clone();
    let clarify_timeout_secs_val = *clarify_timeout_secs_sig.read();
    let timezone_val = timezone_sig.read().clone();
    let write_enabled_val = *write_enabled.read();
    let is_saving = *saving.read();
    let save_error_val = save_error.read().clone();
    let save_ok_val = *save_ok.read();
    let can_save = write_enabled_val && !is_saving;

    rsx! {
        if is_loading {
            for _ in 0..4 {
                div { class: "field-row is-loading", style: "opacity: 0.35;",
                    div { class: "field-label", "\u{00a0}" }
                    div { class: "field-input", style: "height: 22px;" }
                }
            }
        } else if load_failed {
            p { style: "color:var(--red);font-size:12px;margin:0;",
                "Could not load agent config."
            }
            p { style: "color:var(--gray);font-size:11px;margin:4px 0 0 0;",
                "Check the server connection and retry."
            }
        } else {
            div { class: "field-row",
                div { class: "field-label",
                    "Max iterations"
                    span { class: "help", "per-turn budget-handle cap" }
                }
                input {
                    class: "field-input",
                    r#type: "number",
                    min: "1",
                    max: "1000",
                    disabled: !write_enabled_val,
                    value: "{max_iterations_val}",
                    oninput: move |evt| {
                        save_error.set(None);
                        if let Ok(v) = evt.value().parse::<u64>() {
                            max_iterations_sig.set(v);
                        }
                    },
                }
            }
            div { class: "field-row",
                div { class: "field-label",
                    "Context engine"
                    span { class: "help", "compression strategy" }
                }
                select {
                    class: "field-input",
                    disabled: !write_enabled_val,
                    value: "{context_engine_val}",
                    onchange: move |evt| {
                        save_error.set(None);
                        context_engine_sig.set(evt.value());
                    },
                    option { value: "summarizing", "summarizing" }
                    option { value: "local_prune", "local_prune" }
                }
            }
            div { class: "field-row",
                div { class: "field-label",
                    "Context compression"
                    span { class: "help", "ratio [0.0, 1.0] passed to the compression pass" }
                }
                input {
                    class: "field-input",
                    r#type: "number",
                    min: "0",
                    max: "1",
                    step: "0.05",
                    disabled: !write_enabled_val,
                    value: "{context_compression_val}",
                    oninput: move |evt| {
                        save_error.set(None);
                        context_compression_sig.set(evt.value());
                    },
                }
            }
            div { class: "field-row",
                div { class: "field-label",
                    "Compression threshold"
                    span { class: "help", "fraction of context_length at which the loop compresses" }
                }
                input {
                    class: "field-input",
                    r#type: "number",
                    min: "0",
                    max: "1",
                    step: "0.05",
                    disabled: !write_enabled_val,
                    value: "{compression_threshold_val}",
                    oninput: move |evt| {
                        save_error.set(None);
                        compression_threshold_sig.set(evt.value());
                    },
                }
            }
            div { class: "field-row",
                div { class: "field-label",
                    "Tool delay"
                    span { class: "help", "seconds of artificial delay before each tool call" }
                }
                input {
                    class: "field-input",
                    r#type: "number",
                    min: "0",
                    max: "60",
                    step: "0.5",
                    disabled: !write_enabled_val,
                    value: "{tool_delay_secs_val}",
                    oninput: move |evt| {
                        save_error.set(None);
                        tool_delay_secs_sig.set(evt.value());
                    },
                }
            }
            div { class: "field-row",
                div { class: "field-label",
                    "Clarify timeout"
                    span { class: "help", "seconds a suspended clarify tool waits for a response" }
                }
                input {
                    class: "field-input",
                    r#type: "number",
                    min: "1",
                    max: "3600",
                    disabled: !write_enabled_val,
                    value: "{clarify_timeout_secs_val}",
                    oninput: move |evt| {
                        save_error.set(None);
                        if let Ok(v) = evt.value().parse::<u64>() {
                            clarify_timeout_secs_sig.set(v);
                        }
                    },
                }
            }
            div { class: "field-row",
                div { class: "field-label",
                    "Timezone"
                    span { class: "help", "IANA name, e.g. America/Los_Angeles — empty = host local" }
                }
                input {
                    class: "field-input",
                    r#type: "text",
                    disabled: !write_enabled_val,
                    placeholder: "e.g. America/Los_Angeles",
                    value: "{timezone_val}",
                    oninput: move |evt| {
                        save_error.set(None);
                        timezone_sig.set(evt.value());
                    },
                }
            }
            div { class: "field-row",
                div { class: "field-label",
                    "System message"
                    span { class: "help", "optional PRMT-11 system-message slot content" }
                }
                input {
                    class: "field-input",
                    r#type: "text",
                    disabled: !write_enabled_val,
                    value: "{system_message_val}",
                    oninput: move |evt| {
                        save_error.set(None);
                        system_message_sig.set(evt.value());
                    },
                }
            }

            div { class: "field-row",
                button {
                    class: "btn btn--sm",
                    disabled: !can_save,
                    title: if !write_enabled_val { "Config writes are disabled" } else { "" },
                    onclick: move |_| {
                        // Pattern B: owned locals read before spawn.
                        let max_iterations_local = *max_iterations_sig.read() as usize;
                        let context_compression_local = context_compression_sig
                            .read()
                            .trim()
                            .parse::<f64>()
                            .unwrap_or(0.5);
                        let tool_delay_secs_local = tool_delay_secs_sig
                            .read()
                            .trim()
                            .parse::<f64>()
                            .unwrap_or(1.0);
                        let system_message_local = system_message_sig.read().clone();
                        let context_engine_local = context_engine_sig.read().clone();
                        let compression_threshold_local = compression_threshold_sig
                            .read()
                            .trim()
                            .parse::<f32>()
                            .unwrap_or(0.5);
                        let clarify_timeout_secs_local = *clarify_timeout_secs_sig.read();
                        let timezone_raw = timezone_sig.read().clone();
                        let timezone_local = if timezone_raw.trim().is_empty() {
                            String::new()
                        } else {
                            timezone_raw
                        };

                        saving.set(true);
                        save_error.set(None);
                        save_ok.set(false);
                        spawn(async move {
                            let payload = AgentWritePayload {
                                max_iterations: Some(max_iterations_local),
                                context_compression: Some(context_compression_local),
                                tool_delay_secs: Some(tool_delay_secs_local),
                                system_message: Some(system_message_local),
                                context_engine: Some(context_engine_local),
                                compression_threshold: Some(compression_threshold_local),
                                clarify_timeout_secs: Some(clarify_timeout_secs_local),
                                timezone: Some(timezone_local),
                                login_theme: None,
                            };
                            match update_agent_config(payload).await {
                                Ok(()) => {
                                    saving.set(false);
                                    save_ok.set(true);
                                    gloo_timers::future::TimeoutFuture::new(1500).await;
                                    save_ok.set(false);
                                }
                                Err(_e) => {
                                    saving.set(false);
                                    save_error.set(Some("Save failed. Check server logs.".to_string()));
                                }
                            }
                        });
                    },
                    if is_saving {
                        "SAVING…"
                    } else if save_ok_val {
                        "SAVED"
                    } else {
                        "SAVE"
                    }
                }
            }

            if let Some(err) = save_error_val {
                p { style: "color:var(--red);font-size:11px;margin:0;", "{err}" }
            }

            if !write_enabled_val {
                p { class: "help", style: "color:var(--gray);",
                    "Config writes are disabled."
                }
                p { class: "help", style: "color:var(--gray);",
                    "Set security.web_config_write_enabled: true in config.yaml, then retry."
                }
            }
        }
    }
}

/// Phase 47.3 Plan 06 (D-01/D-03): the pre-auth login page's login-theme
/// picker. Follows `AgentConfigPanel`'s exact `write_enabled` signal +
/// `disabled: !write_enabled_val` idiom (settings.rs:387-478) — the same
/// gating pattern, not a new one.
///
/// D-03 hard requirement: when `security.web_config_write_enabled` is false
/// (the default), every one of the five radio inputs carries the TRUE HTML
/// `disabled` attribute (visual dimming alone is a rejected outcome), a lock
/// marker precedes the group, and helper text names the config key to
/// hand-edit. The currently configured option still renders visually checked
/// in that state, reflecting live config — that is what makes the read-only
/// state honest rather than broken.
#[component]
fn LoginThemePanel() -> Element {
    // Slug, display name, and one-line description (lifted verbatim from the
    // design export's own `dv-olabel` prose per UI-SPEC.md's "Settings
    // Login-Theme Picker" section). Order matches D-01's canonical listing:
    // basic (1a, default) / fade (1b) / blast-doors (1c) / matrix-rain (1d)
    // / orbit-veil (2a).
    const THEMES: [(&str, &str, &str); 5] = [
        (
            "basic",
            "Basic",
            "Basic — static terminal auth, no motion beyond the caret",
        ),
        (
            "fade",
            "Fade",
            "Fade features — chrome and preflight checks stage in, then the form",
        ),
        (
            "blast-doors",
            "Blast Doors",
            "Blast doors — lit airlock corridor with a failing lamp; auth splits the bulkhead",
        ),
        (
            "matrix-rain",
            "Matrix Rain",
            "Matrix rain — terminal-dark theme, ANSI green, canvas rain behind the panel",
        ),
        (
            "orbit-veil",
            "Orbit Veil",
            "Auth over leo-live — HUD rebuilt from the app source (ORBIT VEIL: Tailwind slate, rounded-xl, source layer colors); the auth panel deliberately stays IronHermes mono / 0-radius",
        ),
    ];

    let mut loaded = use_signal(|| false);
    let mut write_enabled = use_signal(|| false);
    let mut selected_theme = use_signal(|| "basic".to_string());
    // Separate `use_resource` (not a `use_server_future(...)?` bubbled to the
    // whole screen) so this panel gets its own ghost-loading state,
    // independent of the Agent panel above it — mirrors AgentConfigPanel's
    // own rationale.
    let config_resource = use_resource(move || async move { get_agent_config().await });

    // The picker's labels themselves are fixed compile-time strings (no
    // async load state per must_haves truth) — only the live selection and
    // the write-gate flag come from the resource.
    if !*loaded.read() {
        if let Some(Ok(snapshot)) = config_resource.read().as_ref() {
            write_enabled.set(snapshot.web_config_write_enabled);
            selected_theme.set(snapshot.login_theme.clone());
            loaded.set(true);
        }
    }

    let is_loading = config_resource.read().is_none();
    let load_failed = matches!(config_resource.read().as_ref(), Some(Err(_)));

    let mut saving = use_signal(|| false);
    let mut save_error: Signal<Option<String>> = use_signal(|| None);
    let mut save_ok = use_signal(|| false);

    // `.read()` in render (never `.peek()` — a `.peek()` here would not
    // subscribe, so a later `.set()` would silently fail to re-render).
    let write_enabled_val = *write_enabled.read();
    let selected_theme_val = selected_theme.read().clone();
    let is_saving = *saving.read();
    let save_error_val = save_error.read().clone();
    let save_ok_val = *save_ok.read();

    rsx! {
        if is_loading {
            for _ in 0..3 {
                div { class: "field-row is-loading", style: "opacity: 0.35;",
                    div { class: "field-label", "\u{00a0}" }
                    div { class: "field-input", style: "height: 22px;" }
                }
            }
        } else if load_failed {
            p { style: "color:var(--red);font-size:12px;margin:0;",
                "Could not load login-theme config."
            }
        } else {
            if !write_enabled_val {
                div {
                    class: "field-row",
                    style: "align-items: flex-start; gap: 8px;",
                    span { "aria-hidden": "true", "\u{1F512}" }
                    p { class: "help", style: "margin: 0;",
                        "Read-only — config writes are disabled. Edit web_ui.auth.login_theme in config.yaml to change."
                    }
                }
            }

            for (slug , label , description) in THEMES {
                div { class: "field-row", key: "{slug}",
                    label {
                        style: "display:flex; align-items:flex-start; gap:10px;",
                        input {
                            r#type: "radio",
                            name: "login-theme",
                            value: "{slug}",
                            checked: selected_theme_val == slug,
                            disabled: !write_enabled_val,
                            onchange: move |_| {
                                save_error.set(None);
                                selected_theme.set(slug.to_string());
                                saving.set(true);
                                save_ok.set(false);
                                spawn(async move {
                                    let payload = AgentWritePayload {
                                        max_iterations: None,
                                        context_compression: None,
                                        tool_delay_secs: None,
                                        system_message: None,
                                        context_engine: None,
                                        compression_threshold: None,
                                        clarify_timeout_secs: None,
                                        timezone: None,
                                        login_theme: Some(slug.to_string()),
                                    };
                                    match update_agent_config(payload).await {
                                        Ok(()) => {
                                            saving.set(false);
                                            save_ok.set(true);
                                            gloo_timers::future::TimeoutFuture::new(1500).await;
                                            save_ok.set(false);
                                        }
                                        Err(_e) => {
                                            saving.set(false);
                                            save_error.set(Some("Save failed. Check server logs.".to_string()));
                                        }
                                    }
                                });
                            },
                        }
                        div {
                            div { style: "font-weight:600;", "{label}" }
                            div { class: "help", "{description}" }
                        }
                    }
                }
            }

            if is_saving {
                p { class: "help", style: "color:var(--gray);", "Saving…" }
            } else if save_ok_val {
                p { style: "color:var(--green);font-size:11px;margin:0;", "Saved." }
            }
            if let Some(err) = save_error_val {
                p { style: "color:var(--red);font-size:11px;margin:0;", "{err}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Phase 54.4 Plan 02 (D-05/D-07/D-26): the Settings appearance section no
    //! longer offers wheel-size, Chat rail or Breadcrumb controls. Needles are
    //! assembled from fragments so the test cannot match its own source.

    const SETTINGS_RS: &str = include_str!("settings.rs");

    #[test]
    fn settings_screen_has_no_retired_chrome_controls() {
        for field in ["rail", "breadcrumb", "wheel_size"] {
            let needle = ["p.", field].concat();
            assert!(
                !SETTINGS_RS.contains(&needle),
                "settings.rs still references retired control accessor `{needle}`"
            );
        }
        let kept = ["p.", "footer"].concat();
        assert!(
            SETTINGS_RS.contains(&kept),
            "settings.rs must keep the footer control (`{kept}`)"
        );
    }

    /// The text of `ScreenSettings` only: from its signature to the next
    /// `#[component]`. Needles are assembled from fragments so this region scan
    /// can never match the test source itself.
    fn screen_settings_region() -> &'static str {
        let start_needle = ["pub fn Screen", "Settings("].concat();
        let end_needle = ["#[comp", "onent]"].concat();
        let start = SETTINGS_RS
            .find(&start_needle)
            .expect("ScreenSettings signature present");
        let rest = &SETTINGS_RS[start + start_needle.len()..];
        let end = rest
            .find(&end_needle)
            .expect("a component follows ScreenSettings");
        &SETTINGS_RS[start..start + start_needle.len() + end]
    }

    #[test]
    fn config_yaml_wiring_hint_links_are_buttons() {
        let region = screen_settings_region();
        let old_span = ["span { class: \"help\", \"set via ", "config.yaml\" }"].concat();
        assert_eq!(region.matches(&old_span).count(), 0, "hint spans remain");
        let link = ["cfg-", "hintlink"].concat();
        assert_eq!(region.matches(&link).count(), 3);
    }

    #[test]
    fn config_yaml_wiring_panel_between_kanban_and_ui_preferences() {
        let kanban = SETTINGS_RS
            .find(&["panel-title\", \"Kan", "ban\""].concat())
            .expect("Kanban panel title");
        let mount = SETTINGS_RS
            .find(&["ConfigYaml", "Panel {"].concat())
            .expect("ConfigYamlPanel mount");
        let prefs = SETTINGS_RS
            .find(&["panel-title\", \"UI Pref", "erences\""].concat())
            .expect("UI Preferences panel title");
        assert!(kanban < mount && mount < prefs);
    }

    #[test]
    fn config_yaml_wiring_focus_tick_declared_before_summary_future() {
        let tick = SETTINGS_RS
            .find(&["let mut focus_tick = use_", "signal("].concat())
            .expect("focus_tick signal");
        let future = SETTINGS_RS
            .find(&["use_server_", "future(crate::server::api::get_config_", "summary)"].concat())
            .expect("summary server future");
        assert!(tick < future, "focus_tick must stay above the early return");
    }

    thread_local! {
        static TEST_TAB: std::cell::Cell<super::SettingsTab> =
            const { std::cell::Cell::new(super::SettingsTab::General) };
    }

    fn render_tabs(tab: super::SettingsTab) -> String {
        use dioxus::prelude::*;
        TEST_TAB.with(|t| t.set(tab));

        fn test_app() -> Element {
            let initial = TEST_TAB.with(|t| t.get());
            let active = use_signal(|| initial);
            rsx! { super::SettingsTabs { active } }
        }

        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    /// The opening `<button ...>` tag that carries `id="{id}"`.
    fn button_tag<'a>(html: &'a str, id: &str) -> &'a str {
        let needle = format!("id=\"{id}\"");
        let at = html
            .find(&needle)
            .unwrap_or_else(|| panic!("`{needle}` not in html: {html}"));
        let start = html[..at]
            .rfind("<button")
            .expect("a <button before the id");
        let end = at + html[at..].find('>').expect("tag closes");
        &html[start..=end]
    }

    #[test]
    fn settings_tabs_render_a_three_tab_tablist() {
        for active in super::SETTINGS_TABS {
            let html = render_tabs(active);
            assert_eq!(html.matches("role=\"tablist\"").count(), 1, "{html}");
            assert_eq!(html.matches("role=\"tab\"").count(), 3, "{html}");
            for tab in super::SETTINGS_TABS {
                assert!(html.contains(&format!("id=\"{}\"", tab.tab_id())), "{html}");
                assert!(html.contains(tab.label()), "{html}");
            }
            assert_eq!(html.matches("aria-selected=\"true\"").count(), 1, "{html}");
            assert_eq!(html.matches("aria-selected=\"false\"").count(), 2, "{html}");
            let selected = button_tag(&html, active.tab_id());
            assert!(selected.contains("aria-selected=\"true\""), "{selected}");
            for tab in super::SETTINGS_TABS.into_iter().filter(|t| *t != active) {
                let other = button_tag(&html, tab.tab_id());
                assert!(other.contains("aria-selected=\"false\""), "{other}");
            }
        }
    }

    #[test]
    fn settings_tab_next_prev_wrap() {
        use super::SettingsTab::*;
        assert_eq!(
            (General.next(), Orb.next(), Menu.next()),
            (Orb, Menu, General)
        );
        assert_eq!(
            (General.prev(), Orb.prev(), Menu.prev()),
            (Menu, General, Orb)
        );
    }

    #[test]
    fn settings_tab_signal_precedes_the_server_future() {
        let region = screen_settings_region();
        let signal_at = region
            .find(&["use_sig", "nal("].concat())
            .expect("ScreenSettings declares a tab signal");
        let future_at = region
            .find(&["use_server_", "future("].concat())
            .expect("ScreenSettings loads its summary through a server future");
        assert!(
            signal_at < future_at,
            "the tab signal must be declared before the server future early return"
        );
        let line_start = region[..signal_at].rfind('\n').map_or(0, |i| i + 1);
        let line_end = signal_at + region[signal_at..].find('\n').unwrap_or(0);
        assert!(
            region[line_start..line_end].contains("SettingsTab::General"),
            "the first signal must be the tab signal, defaulting to General"
        );
    }

    #[test]
    fn settings_screen_mounts_voice_and_orb_panels_once() {
        let region = screen_settings_region();
        let voice = ["Voice", "Settings {"].concat();
        let orb = ["OrbSettings", "Panel {"].concat();
        let panel = ["role: \"tab", "panel\""].concat();
        assert_eq!(
            region.matches(&voice).count(),
            1,
            "Voice panel mounted once"
        );
        assert_eq!(region.matches(&orb).count(), 1, "Orb panel mounted once");
        assert_eq!(region.matches(&panel).count(), 3, "one tabpanel per tab");
    }

    const VOICE_MODE_RS: &str = include_str!("voice_mode.rs");

    /// G-54.4-34: the General tab's Voice embed opts out of the orb Appearance
    /// section; the voice-mode overlay mount keeps the default and never names
    /// the prop.
    #[test]
    fn general_voice_mount_opts_out_and_overlay_mount_does_not() {
        let mount = ["Voice", "Settings {"].concat();
        let prop = ["show_orb_", "appearance"].concat();
        let opt_out = [prop.as_str(), ": false"].concat();

        let region = screen_settings_region();
        assert_eq!(
            region.matches(&mount).count(),
            1,
            "General mounts the Voice form once"
        );
        assert_eq!(
            region.matches(&opt_out).count(),
            1,
            "the General mount opts out of the orb appearance exactly once"
        );

        assert_eq!(
            VOICE_MODE_RS.matches(&mount).count(),
            1,
            "positive control: the overlay mounts the Voice form once"
        );
        assert_eq!(
            VOICE_MODE_RS.matches(&prop).count(),
            0,
            "the overlay mount must not name the prop (default true)"
        );
    }
}

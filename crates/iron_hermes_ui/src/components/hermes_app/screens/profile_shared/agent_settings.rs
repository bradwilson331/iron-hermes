//! Phase 54.4 Plan 09 (D-16, D-19, D-22, D-27) — the per-agent settings
//! slide-over.
//!
//! Opened by the gear on an agent tab, it edits a NAMED agent's provider and
//! model and persists them with `update_profile_config`
//! (`ProfileConfigWritePayload`). Edits are STAGED in a working copy: nothing
//! is written until `Save changes`, and `Discard edits` re-seeds the working
//! copy from the last fetch (or the last successful save).
//!
//! Design rules (each one is a past defect in this crate):
//!
//! - A standalone component, mounted by `screens/chat.rs` with `key: agent`,
//!   so a retarget gets a fresh working copy and a fresh fetch (the inline
//!   Soul dropdown revert, 073d4e671, is why this is not bolted onto another
//!   component).
//! - It CONSUMES root contexts (`AgentTabsCtx`, `AgentNoticeCtx`,
//!   `TopbarProfileRefreshCtx`, `Signal<Screen>`) and provides none.
//! - The working copy is seeded ONCE per resolve behind a sentinel, and the
//!   dirty effect only PEEKS the signal it writes — an effect that `.read()`s
//!   what it `.set()`s is a render loop (2aae98af1).
//! - It never raises the APPLY NOW banner: a profile save applies at that
//!   agent's NEXT launch, not to the running server, so the banner would be a
//!   lie.
//! - It never deletes, archives or creates a profile (D-11, D-19): `Close
//!   agent` closes the tab, nothing else.

pub(crate) mod staging;

use crate::components::hermes_app::agent_tabs::{
    AgentNoticeCtx, AgentTabsCtx, AgentTabsState, DEFAULT_AGENT, GuardOutcome, PanelPending,
    focus_agent, record_focus_move, update_tabs_recording_focus,
};
use crate::components::hermes_app::header::focus_element_by_id;
use crate::components::hermes_app::profile_topbar::TopbarProfileRefreshCtx;
use crate::components::hermes_app::screens::model_picker::ModelPickerField;
use crate::protocol::{ProfileConfigWritePayload, ProfileSkillRow, RootAgentDefaults};
use crate::server::api::{ProviderModelsSnapshot, list_provider_models};
use crate::server::profile_api::{
    fetch_profile_detail, fetch_profile_skills, fetch_root_agent_defaults, update_profile_config,
};
use crate::server::provider_config_api::get_provider_config;
use crate::server::tools_config_api::{ConfigScope, ToolsetKind, get_tools_page_state};
use crate::state::Screen;
use dioxus::prelude::*;
use staging::{
    SelectionRow, SkillsStage, ToolsPayload, ToolsStage, apply_root_defaults, disable_skill,
    disable_tool, disable_toolset, enable_skill, enable_tool, enable_toolset, keep_when_unset,
    skill_chips, skill_selection, skills_payload, skills_stage_from_rows, tool_selection,
    tool_selection_filtered, tools_payload, tools_stage_from_page, toolset_chips,
};
use std::collections::BTreeSet;

/// The field hint shown when a profile stores no provider/model (UIC
/// E10-empty): the agent inherits the root default.
const INHERITS_DEFAULT: &str = "(inherits default)";

/// The value to send for a provider/model field (CR-01): `Some` only when the
/// operator actually set a non-blank value that differs from the seed. A blank
/// field is the "(inherits default)" state, and an unchanged field is not part
/// of the edit, so both send `None` and the server leaves the key alone. The
/// server keeps rejecting an explicit empty string.
fn changed_field(new: &str, old: &str) -> Option<String> {
    let new = new.trim();
    if new.is_empty() || new == old.trim() {
        None
    } else {
        Some(new.to_string())
    }
}

/// The ONE save request for the staged form (D-19): provider, model and only
/// the sections that changed. A section whose data failed to load is passed as
/// `None` so the server leaves it alone. Provider and model are sent only when
/// changed from `seed` and non-blank (CR-01), so a profile that inherits both
/// can still save a tools-only or skills-only edit.
pub(crate) fn build_save_payload(
    agent: &str,
    provider: &str,
    model: &str,
    seed: &(String, String),
    tools: ToolsPayload,
    skills_disabled: Option<Vec<String>>,
) -> ProfileConfigWritePayload {
    let (tools_toolsets, tools_disable, tools_enable) = tools;
    ProfileConfigWritePayload {
        name: agent.to_string(),
        provider: changed_field(provider, &seed.0),
        model_default: changed_field(model, &seed.1),
        skills_disabled,
        tools_toolsets,
        tools_disable,
        tools_enable,
    }
}

/// Whether the working copy `wc` differs from the seed (provider, model).
pub(crate) fn is_dirty(seed: &(String, String), wc: &(String, String)) -> bool {
    seed != wc
}

/// The provider select's option list: the configured providers, with a stale
/// assigned provider (one no longer configured) prepended so it never vanishes
/// from its own dropdown. A blank `assigned` prepends nothing.
pub(crate) fn provider_choices(configured: &[String], assigned: &str) -> Vec<String> {
    let mut options: Vec<String> = configured.to_vec();
    let assigned = assigned.trim();
    if !assigned.is_empty() && !options.iter().any(|o| o == assigned) {
        options.insert(0, assigned.to_string());
    }
    options
}

/// UI-SPEC "Save error" copy.
pub(crate) fn save_error_copy(agent: &str, reason: &str) -> String {
    let reason = reason.trim().trim_end_matches('.');
    format!(
        "Couldn't save {agent}'s settings: {reason}. Your edits are still here. Fix the problem and press Save changes again."
    )
}

/// UI-SPEC "Save success" copy.
pub(crate) fn save_success_copy(agent: &str) -> String {
    format!("Saved. Applies to {agent}'s next launch.")
}

/// UI-SPEC scope note.
pub(crate) fn scope_note(agent: &str) -> String {
    format!("Saved changes apply the next time {agent} launches. The running chat is not affected.")
}

/// UI-SPEC "Write gate closed" copy.
pub(crate) const GATE_CLOSED_COPY: &str = "Saving is turned off on this server (security.web_config_write_enabled is off). You can review settings but not save them.";

/// The confirmation bar's `(question, confirm button label)` for a parked
/// action (UI-SPEC Copywriting: "Destructive confirmation"). Closing the
/// edited agent names the agent being closed; every other action discards the
/// panel agent's edits.
pub(crate) fn confirm_copy(pending: &PanelPending, panel_agent: &str) -> (String, &'static str) {
    match pending {
        PanelPending::Close(agent) => (
            format!(
                "Close agent: Close {agent}? You have unsaved settings changes that will be lost."
            ),
            "Close agent",
        ),
        PanelPending::Focus(_) | PanelPending::Hide | PanelPending::Navigate(_) => (
            format!("Discard changes: Discard unsaved changes to {panel_agent}?"),
            "Discard changes",
        ),
    }
}

/// Hide the panel through the dirty guard and, when it hides at once, return
/// focus to the gear that opened it (UI-SPEC focus rule).
fn hide_and_return_focus(mut tabs: Signal<AgentTabsState>, agent: &str) {
    let outcome = tabs.with_mut(|s| s.request_hide());
    if outcome == GuardOutcome::Applied {
        focus_element_by_id(&format!("ih-tab-gear-{agent}"));
    }
}

/// The screen a cancelled action puts back: only a held navigation changed
/// the router's requested screen, so only it restores Chat (the header's
/// MODULE then matches what is mounted).
pub(crate) fn keep_editing_screen(cancelled: Option<&PanelPending>) -> Option<Screen> {
    match cancelled {
        Some(PanelPending::Navigate(_)) => Some(Screen::Chat),
        _ => None,
    }
}

/// The operator chose to keep editing (the Keep editing button and Esc in the
/// confirmation bar both end here, so they cannot drift apart): drop the
/// parked action, put Chat back for a held navigation, and move focus to the
/// panel title.
///
/// The bar held focus and unmounts once the action is cleared, so focus would
/// otherwise fall to the document body, outside the aside's keydown scope. The
/// title is the anchor `PanelTitleBar` already focuses on open, and unlike the
/// MODEL input it is valid whichever control started the hold (header
/// shortcut, MENU, wheel wedge or the title-bar close control).
fn keep_editing(mut tabs: Signal<AgentTabsState>, mut screen: Signal<Screen>) {
    let cancelled = tabs.with_mut(|s| s.cancel_pending());
    if let Some(target) = keep_editing_screen(cancelled.as_ref()) {
        screen.set(target);
    }
    focus_element_by_id("ih-slideover-title");
}

/// Which variant of the panel an agent gets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PanelMode {
    /// A named profile: staged provider/model edits, Save / Discard.
    Editable,
    /// The root `default` agent (D-22): read-only, links out to the screens
    /// that own the root config. Never writes the root config.yaml.
    ReadOnlyRoot,
}

/// The panel variant for `agent`: only the reserved root id is read-only.
pub(crate) fn panel_mode(agent: &str) -> PanelMode {
    if agent == DEFAULT_AGENT {
        PanelMode::ReadOnlyRoot
    } else {
        PanelMode::Editable
    }
}

/// The root agent's four read-only rows (D-22): provider, model and the
/// COUNTS of disabled skills and disabled tools ("Disabled tools" adds the
/// disabled built-in toolsets to the individually disabled tools). An unset
/// provider or model reads as an em dash.
pub(crate) fn root_rows(d: &RootAgentDefaults) -> Vec<(&'static str, String)> {
    let show = |v: &str| {
        if v.trim().is_empty() {
            "\u{2014}".to_string()
        } else {
            v.to_string()
        }
    };
    vec![
        ("Provider", show(&d.provider)),
        ("Model", show(&d.model)),
        ("Disabled skills", d.skills_disabled.len().to_string()),
        (
            "Disabled tools",
            (d.disabled_toolsets.len() + d.tools_disabled.len()).to_string(),
        ),
    ]
}

/// Whether `Save changes` is enabled: there are edits, the server's write gate
/// is open, no save is in flight and the config has finished loading.
pub(crate) fn save_enabled(dirty: bool, gate_open: bool, saving: bool, loading: bool) -> bool {
    dirty && gate_open && !saving && !loading
}

/// UI-SPEC read-only default agent note (D-22).
const ROOT_NOTE: &str =
    "This is the root profile. Change its provider, model and tools on their own screens.";

/// Hide the settings panel and leave Chat for `target`. The root panel is
/// never dirty, so nothing needs confirming; hiding first stops the panel
/// from reappearing when the operator comes back to Chat.
fn leave_chat_for(mut tabs: Signal<AgentTabsState>, mut screen: Signal<Screen>, target: Screen) {
    tabs.with_mut(|s| s.hide_settings());
    screen.set(target);
}

/// The panel's title bar: the title (it takes focus once, on open), the amber
/// unsaved dot and the Hide control (hides the PANEL only, never closes the
/// agent). Hiding goes through the dirty guard.
#[component]
fn PanelTitleBar(agent: String, dirty: bool) -> Element {
    let tabs = use_context::<AgentTabsCtx>().0;
    let hide_agent = agent.clone();

    // Mount focus: the title takes focus once, on open.
    let mut focused_once = use_signal(|| false);
    use_effect(move || {
        if !*focused_once.peek() {
            focused_once.set(true);
            focus_element_by_id("ih-slideover-title");
        }
    });

    rsx! {
        div { class: "ih-slideover-titlebar",
            h2 {
                id: "ih-slideover-title",
                class: "ih-slideover-title",
                tabindex: "-1",
                title: "{agent} settings",
                "{agent} settings"
            }
            if dirty {
                span {
                    class: "ih-unsaved-dot",
                    role: "img",
                    "aria-label": "Unsaved changes",
                    title: "Unsaved changes",
                }
            }
            button {
                r#type: "button",
                class: "ih-slideover-hide",
                "aria-label": "Hide settings",
                title: "Hide settings",
                onclick: move |_| hide_and_return_focus(tabs, &hide_agent),
                "\u{2715}"
            }
        }
    }
}

/// The confirmation bar for a parked action (D-16, UI-SPEC "Destructive
/// confirmation").
///
/// It lives OUTSIDE the scrolling `.ih-slideover-body`, as a direct child of
/// the aside between the title bar and the body (G-54.4-5): rendered as the
/// body's first child, a body scrolled by typing in MODEL plus Chrome's scroll
/// anchoring carried it ~150px above the visible area, so the operator saw a
/// held navigation and no prompt.
///
/// It CONSUMES root contexts (`AgentTabsCtx`, `AgentNoticeCtx`,
/// `TopbarProfileRefreshCtx`, `Signal<Screen>`) and provides none.
#[component]
fn ConfirmBar(agent: String, pending: PanelPending) -> Element {
    let mut tabs = use_context::<AgentTabsCtx>().0;
    let notice = use_context::<AgentNoticeCtx>().0;
    let refresh = use_context::<TopbarProfileRefreshCtx>().0;
    let screen = use_context::<Signal<Screen>>();

    // Owned locals BEFORE rsx! — no signal borrow is held across the tree.
    let (question, confirm_label) = confirm_copy(&pending, &agent);
    let pending_is_hide = matches!(pending, PanelPending::Hide);
    let confirm_agent = agent.clone();

    rsx! {
        div {
            class: "ih-confirm-bar",
            role: "alertdialog",
            "aria-labelledby": "ih-confirm-text",
            // Esc is Keep editing. It stops here: the aside's own Esc handler
            // would request a Hide and re-park it over a held navigation.
            onkeydown: move |e: KeyboardEvent| {
                if e.key() == Key::Escape {
                    e.stop_propagation();
                    keep_editing(tabs, screen);
                }
            },
            span { id: "ih-confirm-text", class: "ih-confirm-text", "{question}" }
            div { class: "ih-confirm-actions",
                button {
                    r#type: "button",
                    class: "ih-btn-danger",
                    onclick: move |_| {
                        // A confirmed Focus or Close moves focus; record the
                        // new focus (WR-01/WR-02). The write runs on the root
                        // scope, so it survives this panel re-keying away.
                        let before = tabs.peek().focused.clone();
                        tabs.with_mut(|s| s.confirm_pending());
                        let after = tabs.peek().focused.clone();
                        record_focus_move(
                            notice,
                            refresh,
                            before.as_deref(),
                            after.as_deref(),
                        );
                        if pending_is_hide {
                            focus_element_by_id(&format!("ih-tab-gear-{confirm_agent}"));
                        }
                    },
                    "{confirm_label}"
                }
                button {
                    r#type: "button",
                    class: "ih-btn-secondary",
                    id: "ih-confirm-keep",
                    // The bar mounts each time an action is parked. Focus lands
                    // on the safe action (WAI-ARIA alertdialog guidance for a
                    // destructive confirmation), so a stray Enter keeps the
                    // edits. Same grab as command_palette.rs.
                    onmounted: move |e: Event<MountedData>| {
                        spawn(async move {
                            let _ = e.data().set_focus(true).await;
                        });
                    },
                    onclick: move |_| keep_editing(tabs, screen),
                    "Keep editing"
                }
            }
        }
    }
}

/// D-16: the pointer that replaces the profile drawer's provider/model editor
/// and the Advanced pane's skills checklist. It opens `agent`'s tab AND its
/// settings slide-over on Chat, through the same dirty guard as every other
/// focus change (a dirty panel for another agent parks the switch behind its
/// confirm bar instead). One implementation for both mount sites; `label` is
/// passed in so each site owns its own visible copy.
///
/// It CONSUMES root contexts only (`AgentTabsCtx`, `AgentNoticeCtx`,
/// `TopbarProfileRefreshCtx`, `Signal<Screen>`) and provides none.
#[component]
pub fn OpenAgentSettingsButton(agent: String, label: String) -> Element {
    let mut tabs = use_context::<AgentTabsCtx>().0;
    let notice = use_context::<AgentNoticeCtx>().0;
    let refresh = use_context::<TopbarProfileRefreshCtx>().0;
    let mut screen = use_context::<Signal<Screen>>();

    rsx! {
        button {
            r#type: "button",
            class: "btn btn--ghost btn--sm",
            onclick: move |_| {
                tabs.with_mut(|s| {
                    s.open_settings(&agent);
                });
                focus_agent(tabs, notice, refresh, agent.clone());
                screen.set(Screen::Chat);
            },
            "{label}"
        }
    }
}

/// The per-agent settings slide-over. Mount with `key: agent` so a retarget
/// gets a fresh working copy. The root `default` agent gets the read-only
/// variant (D-22); every named profile gets the editable one.
#[component]
pub fn AgentSettingsPanel(agent: String) -> Element {
    match panel_mode(&agent) {
        PanelMode::ReadOnlyRoot => rsx! { RootAgentPanel { agent } },
        PanelMode::Editable => rsx! { EditableAgentPanel { agent } },
    }
}

/// D-22: the root agent's read-only panel. Four rows from
/// `fetch_root_agent_defaults` (a read), links to the screens that own the
/// root config, and a `Close agent` footer. There is no working copy, no
/// staged edit and no write of any kind in this component.
#[component]
fn RootAgentPanel(agent: String) -> Element {
    let tabs = use_context::<AgentTabsCtx>().0;
    let notice = use_context::<AgentNoticeCtx>().0;
    let refresh = use_context::<TopbarProfileRefreshCtx>().0;
    let screen = use_context::<Signal<Screen>>();
    let mut retry_tick = use_signal(|| 0_u32);
    let defaults = use_resource(move || {
        let _tick = *retry_tick.read();
        async move { fetch_root_agent_defaults().await }
    });

    let state = defaults();
    let (rows, load_error) = match &state {
        Some(Ok(d)) => (root_rows(d), None),
        Some(Err(e)) => (Vec::new(), Some(format!("{e}"))),
        None => (Vec::new(), None),
    };
    let loading = state.is_none();

    let esc_agent = agent.clone();
    let close_agent = agent.clone();

    rsx! {
        aside {
            class: "ih-slideover",
            role: "dialog",
            "aria-modal": "false",
            "aria-labelledby": "ih-slideover-title",
            onkeydown: move |e: KeyboardEvent| {
                if e.key() == Key::Escape {
                    hide_and_return_focus(tabs, &esc_agent);
                }
            },
            PanelTitleBar { agent: agent.clone(), dirty: false }
            div { class: "ih-slideover-body",
                div { class: "ih-slideover-note", "{ROOT_NOTE}" }
                if let Some(ref reason) = load_error {
                    div { class: "ih-slideover-error", role: "alert",
                        "Couldn't load {agent}'s settings: {reason}."
                        button {
                            r#type: "button",
                            class: "ih-btn-secondary",
                            onclick: move |_| {
                                let cur = *retry_tick.peek();
                                retry_tick.set(cur + 1);
                            },
                            "Retry now"
                        }
                    }
                }
                if loading {
                    div { class: "ih-slideover-note", "Loading\u{2026}" }
                }
                dl { class: "ih-root-rows",
                    for (label , value) in rows.iter() {
                        div { class: "ih-root-row", key: "{label}",
                            dt { class: "ih-slideover-label", "{label}" }
                            dd { class: "ih-root-value", title: "{value}", "{value}" }
                        }
                    }
                }
                div { class: "ih-root-links",
                    button {
                        r#type: "button",
                        class: "ih-btn-secondary",
                        onclick: move |_| leave_chat_for(tabs, screen, Screen::Providers),
                        "Open Providers"
                    }
                    button {
                        r#type: "button",
                        class: "ih-btn-secondary",
                        onclick: move |_| leave_chat_for(tabs, screen, Screen::Models),
                        "Open Models"
                    }
                    button {
                        r#type: "button",
                        class: "ih-btn-secondary",
                        onclick: move |_| leave_chat_for(tabs, screen, Screen::Tools),
                        "Open Tools"
                    }
                }
            }
            div { class: "ih-slideover-footer",
                div { class: "ih-slideover-footer-row",
                    button {
                        r#type: "button",
                        class: "ih-btn-danger",
                        onclick: move |_| {
                            update_tabs_recording_focus(tabs, notice, refresh, |s| {
                                s.request_close(&close_agent)
                            });
                        },
                        "Close agent"
                    }
                    span { class: "ih-slideover-helper",
                        "Closes this agent's tab. The profile and its files are kept."
                    }
                }
            }
        }
    }
}

/// One ENABLED item (D-17): a chip. `x` (or Backspace / Delete on the focused
/// chip) disables it. A toolset chip also carries a right-pointing triangle
/// expand control that reveals its member tool chips (D-18). Presentational
/// only: the caller owns the staging state, so removing a chip writes nothing.
#[component]
fn Chip(
    label: String,
    remove_label: String,
    disabled: bool,
    on_remove: EventHandler<()>,
    #[props(default)] expandable: bool,
    #[props(default)] expanded: bool,
    #[props(default)] expand_label: String,
    #[props(default)] on_toggle: EventHandler<()>,
) -> Element {
    rsx! {
        span {
            class: "ih-chip",
            title: "{label}",
            tabindex: "0",
            onkeydown: move |e: KeyboardEvent| {
                if matches!(e.key(), Key::Backspace | Key::Delete) {
                    e.prevent_default();
                    if !disabled {
                        on_remove.call(());
                    }
                }
            },
            if expandable {
                button {
                    r#type: "button",
                    class: "ih-chip-expand",
                    "aria-label": "{expand_label}",
                    "aria-expanded": "{expanded}",
                    onclick: move |_| on_toggle.call(()),
                    if expanded {
                        "\u{25BE}"
                    } else {
                        "\u{25B8}"
                    }
                }
            }
            span { class: "ih-chip-label", "{label}" }
            button {
                r#type: "button",
                class: "ih-chip-remove",
                "aria-label": "{remove_label}",
                disabled,
                onclick: move |_| on_remove.call(()),
                "\u{00D7}"
            }
        }
    }
}

/// One re-addable row in a selection list: 40px, the name, a one-line clamped
/// detail and an `Add ...` button.
#[component]
fn SelectionItem(
    label: String,
    detail: String,
    add_label: String,
    add_aria: String,
    disabled: bool,
    on_add: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "ih-select-row",
            div { class: "ih-select-text",
                span { class: "ih-select-name", title: "{label}", "{label}" }
                if !detail.is_empty() {
                    span { class: "ih-select-desc", title: "{detail}", "{detail}" }
                }
            }
            button {
                r#type: "button",
                class: "ih-btn-secondary ih-select-add",
                "aria-label": "{add_aria}",
                title: "{add_aria}",
                disabled,
                onclick: move |_| on_add.call(()),
                "{add_label}"
            }
        }
    }
}

/// The editable panel for a named (non-root) agent.
#[component]
fn EditableAgentPanel(agent: String) -> Element {
    let mut tabs = use_context::<AgentTabsCtx>().0;
    let notice = use_context::<AgentNoticeCtx>().0;
    let refresh = use_context::<TopbarProfileRefreshCtx>().0;

    // ALL hooks register unconditionally on every render.
    let mut retry_tick = use_signal(|| 0_u32);
    let mut provider_wc = use_signal(String::new);
    let mut model_wc = use_signal(String::new);
    let mut seed = use_signal(|| (String::new(), String::new()));
    let mut seeded_once = use_signal(|| false);
    let mut saving = use_signal(|| false);
    let mut save_msg = use_signal(|| None::<Result<String, String>>);

    let detail_agent = agent.clone();
    let detail = use_resource(move || {
        let _tick = *retry_tick.read();
        let name = detail_agent.clone();
        async move { fetch_profile_detail(name).await }
    });
    let providers = use_resource(move || async move { get_provider_config().await });
    // The model list is fetched only while this panel is mounted, and only
    // once the working copy is seeded (an empty provider returns the whole
    // ~300-model fallback catalog).
    let models = use_resource(move || {
        let ready = *seeded_once.read();
        let provider = provider_wc();
        async move {
            if !ready {
                return Ok(ProviderModelsSnapshot {
                    models: Vec::new(),
                    fell_back: false,
                });
            }
            list_provider_models(provider).await
        }
    });

    // D-17/D-18: the profile-scoped Tools page state feeds the toolset chips.
    // Its staged selection is seeded ONCE per resolve (sentinel), exactly like
    // provider/model; a retry tick re-runs only a failed fetch.
    let mut tools_tick = use_signal(|| 0_u32);
    let mut tools_stage = use_signal(ToolsStage::default);
    let mut tools_seed = use_signal(ToolsStage::default);
    let mut tools_seeded = use_signal(|| false);
    let mut expanded = use_signal(BTreeSet::<String>::new);
    let mut tools_query = use_signal(String::new);
    let tools_agent = agent.clone();
    let tools = use_resource(move || {
        let _tick = *tools_tick.read();
        let name = tools_agent.clone();
        async move { get_tools_page_state(ConfigScope::Profile(name)).await }
    });

    // D-17/D-20: the profile's skills catalog (already intersected with the
    // live catalog server-side, so a stale `skills.disabled` name never
    // appears). Same sentinel-seeded shape as the tools section.
    let mut skills_tick = use_signal(|| 0_u32);
    let mut skills_stage = use_signal(SkillsStage::default);
    let mut skills_seed = use_signal(SkillsStage::default);
    let mut skills_seeded = use_signal(|| false);
    let mut skills_query = use_signal(String::new);
    let skills_agent = agent.clone();
    let skills = use_resource(move || {
        let _tick = *skills_tick.read();
        let name = skills_agent.clone();
        async move { fetch_profile_skills(name).await }
    });

    // Seed the working copy ONCE per resolve. Only `detail` is subscribed;
    // every signal written here is peeked or write-only.
    use_effect(move || {
        if let Some(Ok(d)) = detail() {
            if !*seeded_once.peek() {
                let p = d.provider.clone().unwrap_or_default();
                let m = d.model_default.clone().unwrap_or_default();
                provider_wc.set(p.clone());
                model_wc.set(m.clone());
                seed.set((p, m));
                seeded_once.set(true);
            }
        }
    });

    // Seed the staged tools selection ONCE per resolve. Subscribes to the
    // resource only; the signals it writes are peeked or write-only.
    use_effect(move || {
        if let Some(Ok(page)) = tools() {
            if !*tools_seeded.peek() {
                let staged = tools_stage_from_page(&page);
                tools_stage.set(staged.clone());
                tools_seed.set(staged);
                tools_seeded.set(true);
            }
        }
    });

    // Seed the staged skills opt-out ONCE per resolve (same discipline).
    use_effect(move || {
        if let Some(Ok(rows)) = skills() {
            if !*skills_seeded.peek() {
                let staged = skills_stage_from_rows(&rows);
                skills_stage.set(staged.clone());
                skills_seed.set(staged);
                skills_seeded.set(true);
            }
        }
    });

    // Mirror dirtiness into the tab model so the guards (Task 2) can see it.
    // Subscribes to the working copy and the seed; PEEKS the tab model, and
    // writes only on a change, so it cannot retrigger itself.
    use_effect(move || {
        let wc = (provider_wc.read().clone(), model_wc.read().clone());
        let sd = seed.read().clone();
        let now_dirty = is_dirty(&sd, &wc)
            || *tools_stage.read() != *tools_seed.read()
            || *skills_stage.read() != *skills_seed.read();
        if tabs.peek().panel.dirty != now_dirty {
            tabs.with_mut(|s| s.panel.dirty = now_dirty);
        }
    });

    // Owned locals BEFORE rsx! — no signal borrow is held across the tree.
    let dirty = is_dirty(
        &seed.read().clone(),
        &(provider_wc.read().clone(), model_wc.read().clone()),
    ) || *tools_stage.read() != *tools_seed.read()
        || *skills_stage.read() != *skills_seed.read();
    let provider_val = provider_wc.read().clone();
    let model_val = model_wc.read().clone();
    let model_seed = seed.read().1.clone();
    let is_saving = *saving.read();
    let seeded = *seeded_once.read();
    let detail_state = detail();
    let (gate_open, fetch_error) = match &detail_state {
        Some(Ok(d)) => (d.web_config_write_enabled, None),
        Some(Err(e)) => (false, Some(format!("{e}"))),
        None => (false, None),
    };
    let loading = !seeded && fetch_error.is_none();
    let provider_names: Vec<String> = match providers() {
        Some(Ok(snap)) => snap.providers.iter().map(|p| p.name.clone()).collect(),
        _ => Vec::new(),
    };
    let snapshot = match models() {
        Some(Ok(snap)) => Some(snap),
        _ => None,
    };
    let models_loading = models().is_none();
    let fell_back = snapshot.as_ref().is_some_and(|s| s.fell_back);
    let assigned_model = if model_val.trim().is_empty() {
        None
    } else {
        Some(model_val.as_str())
    };
    let model_options = crate::components::hermes_app::screens::models::compute_model_options(
        snapshot.as_ref(),
        assigned_model,
    );
    let choices = provider_choices(&provider_names, &provider_val);
    let save_disabled = !save_enabled(dirty, gate_open, is_saving, loading);
    let discard_disabled = !dirty || loading || is_saving;
    let fields_disabled = loading || is_saving || fetch_error.is_some();
    let save_msg_val = save_msg.read().clone();
    let save_ok = save_msg_val.clone().and_then(Result::ok);
    let save_err = save_msg_val.and_then(Result::err);

    // Tools section (D-17/D-18): owned locals, no signal borrow in the tree.
    let tools_state = tools();
    let tools_error = match &tools_state {
        Some(Err(e)) => Some(format!("{e}")),
        _ => None,
    };
    let tools_page = match tools_state {
        Some(Ok(page)) => Some(page),
        _ => None,
    };
    let tools_ready = tools_page.is_some() && *tools_seeded.read();
    let stage_now = tools_stage.read().clone();
    let (tool_chip_rows, tool_rows_all, has_mcp) = match (&tools_page, tools_ready) {
        (Some(page), true) => (
            toolset_chips(&stage_now, page),
            tool_selection(&stage_now, page),
            page.toolsets
                .iter()
                .any(|g| matches!(g.kind, ToolsetKind::Mcp { .. })),
        ),
        _ => (Vec::new(), Vec::new(), false),
    };
    let tools_query_now = tools_query.read().clone();
    let tool_rows = tool_selection_filtered(&tool_rows_all, tools_query_now.trim());
    let tools_no_match =
        !tools_query_now.trim().is_empty() && tool_rows.is_empty() && !tool_rows_all.is_empty();
    let expanded_now = expanded.read().clone();

    // Skills section (D-17/D-20), same shape: owned locals only.
    let skills_state = skills();
    let skills_error = match &skills_state {
        Some(Err(e)) => Some(format!("{e}")),
        _ => None,
    };
    let skills_rows: Option<Vec<ProfileSkillRow>> = match skills_state {
        Some(Ok(rows)) => Some(rows),
        _ => None,
    };
    let skills_ready = skills_rows.is_some() && *skills_seeded.read();
    let skills_stage_now = skills_stage.read().clone();
    let skills_query_now = skills_query.read().clone();
    let skill_catalog: &[ProfileSkillRow] = skills_rows.as_deref().unwrap_or(&[]);
    let (skill_chip_names, skill_off_all, skill_off) = if skills_ready {
        (
            skill_chips(&skills_stage_now, skill_catalog),
            skill_selection(&skills_stage_now, skill_catalog, ""),
            skill_selection(&skills_stage_now, skill_catalog, skills_query_now.trim()),
        )
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };
    let skills_empty_catalog = skills_ready && skill_catalog.is_empty();
    let skills_no_match =
        !skills_query_now.trim().is_empty() && skill_off.is_empty() && !skill_off_all.is_empty();

    // What Load default folds in: only sections whose data actually loaded.
    let defaults_page = if tools_ready {
        tools_page.clone()
    } else {
        None
    };
    let defaults_rows = if skills_ready {
        skills_rows.clone()
    } else {
        None
    };

    // The parked action awaiting confirmation, as owned locals.
    let pending = tabs.read().panel.pending.clone();

    let save_agent = agent.clone();
    let close_agent = agent.clone();
    let esc_agent = agent.clone();

    rsx! {
        aside {
            class: "ih-slideover",
            role: "dialog",
            "aria-modal": "false",
            "aria-labelledby": "ih-slideover-title",
            onkeydown: move |e: KeyboardEvent| {
                if e.key() == Key::Escape {
                    hide_and_return_focus(tabs, &esc_agent);
                }
            },
            PanelTitleBar { agent: agent.clone(), dirty }
            // G-54.4-5: a sibling of the scrolling body, never inside it.
            if let Some(p) = pending.clone() {
                ConfirmBar { agent: agent.clone(), pending: p }
            }
            div { class: "ih-slideover-body",
                if let Some(ref reason) = fetch_error {
                    div { class: "ih-slideover-error", role: "alert",
                        "Couldn't load {agent}'s settings: {reason}."
                        button {
                            r#type: "button",
                            class: "ih-btn-secondary",
                            onclick: move |_| {
                                let cur = *retry_tick.peek();
                                retry_tick.set(cur + 1);
                            },
                            "Retry now"
                        }
                    }
                } else if !gate_open && !loading {
                    div { class: "ih-slideover-note", "{GATE_CLOSED_COPY}" }
                } else {
                    div { class: "ih-slideover-note", "{scope_note(&agent)}" }
                }
                div { class: "ih-slideover-section",
                    label {
                        class: "ih-slideover-label",
                        r#for: "ih-slideover-provider",
                        "PROVIDER"
                    }
                    select {
                        id: "ih-slideover-provider",
                        class: "voice-settings-select",
                        disabled: fields_disabled,
                        onchange: move |evt| {
                            provider_wc.set(evt.value());
                            save_msg.set(None);
                        },
                        option {
                            value: "",
                            selected: provider_val.trim().is_empty(),
                            "{INHERITS_DEFAULT}"
                        }
                        for name in choices.iter() {
                            option {
                                key: "{name}",
                                value: "{name}",
                                selected: name == &provider_val,
                                "{name}"
                            }
                        }
                    }
                }
                div { class: "ih-slideover-section",
                    label { class: "ih-slideover-label", "MODEL" }
                    // Esc inside the model field closes its popup; it must not
                    // also hide the whole panel (and with it the edits).
                    div {
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Escape {
                                e.stop_propagation();
                            }
                        },
                        ModelPickerField {
                            value: model_wc,
                            all_options: model_options,
                            seeded: model_seed,
                            disabled: fields_disabled || models_loading,
                            placeholder: INHERITS_DEFAULT.to_string(),
                            title: model_val.clone(),
                        }
                    }
                    if fell_back && !models_loading {
                        div { class: "ih-slideover-note",
                            "This provider exposes no model list \u{2014} showing the full catalog."
                        }
                    }
                }
                div { class: "ih-slideover-section",
                    label { class: "ih-slideover-label", "TOOLS" }
                    if let Some(ref reason) = tools_error {
                        div { class: "ih-slideover-error", role: "alert",
                            "Couldn't load tools: {reason}"
                            button {
                                r#type: "button",
                                class: "ih-btn-secondary",
                                onclick: move |_| {
                                    let cur = *tools_tick.peek();
                                    tools_tick.set(cur + 1);
                                },
                                "Retry now"
                            }
                        }
                    }
                    div { class: "ih-chip-box",
                        if !tools_ready && tools_error.is_none() {
                            div { class: "ih-slideover-note", "Loading tools\u{2026}" }
                        }
                        if tools_ready && tool_chip_rows.is_empty() {
                            div { class: "ih-slideover-note", "All tools are turned off for {agent}." }
                        }
                        for chip in tool_chip_rows.iter() {
                            {
                                let name = chip.name.clone();
                                let is_open = expanded_now.contains(&chip.name);
                                let toggle_name = chip.name.clone();
                                let remove_name = chip.name.clone();
                                rsx! {
                                    div { class: "ih-chip-group", key: "{name}",
                                        Chip {
                                            label: name.clone(),
                                            remove_label: format!("Disable {name}"),
                                            disabled: is_saving,
                                            on_remove: move |_| {
                                                tools_stage.with_mut(|s| {
                                                    disable_toolset(s, &remove_name);
                                                });
                                                save_msg.set(None);
                                            },
                                            expandable: true,
                                            expanded: is_open,
                                            expand_label: format!("Show tools in {name}"),
                                            on_toggle: move |_| {
                                                expanded.with_mut(|set| {
                                                    if !set.remove(&toggle_name) {
                                                        set.insert(toggle_name.clone());
                                                    }
                                                });
                                            },
                                        }
                                        if is_open {
                                            div { class: "ih-chip-members",
                                                for tool in chip.tools.iter() {
                                                    {
                                                        let tool_name = tool.name.clone();
                                                        let member_name = tool.name.clone();
                                                        rsx! {
                                                            Chip {
                                                                key: "{tool_name}",
                                                                label: tool_name.clone(),
                                                                remove_label: format!("Disable {tool_name}"),
                                                                disabled: is_saving,
                                                                on_remove: move |_| {
                                                                    tools_stage.with_mut(|s| {
                                                                        disable_tool(s, &member_name);
                                                                    });
                                                                    save_msg.set(None);
                                                                },
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    input {
                        r#type: "search",
                        class: "ih-search",
                        placeholder: "Search tools",
                        "aria-label": "Search tools",
                        disabled: !tools_ready,
                        value: "{tools_query_now}",
                        oninput: move |e| tools_query.set(e.value()),
                    }
                    if !tool_rows_all.is_empty() {
                        div { class: "ih-slideover-note", "Tool selection" }
                    }
                    div {
                        class: "ih-select-list",
                        role: "list",
                        "aria-label": "Tool selection",
                        for row in tool_rows.iter() {
                            {
                                let (key, label, detail, add_name) = match row {
                                    SelectionRow::Toolset { name } => (
                                        format!("set:{name}"),
                                        name.clone(),
                                        "Toolset".to_string(),
                                        name.clone(),
                                    ),
                                    SelectionRow::Tool { toolset, name, description } => (
                                        format!("tool:{name}"),
                                        name.clone(),
                                        format!("{toolset}: {description}"),
                                        name.clone(),
                                    ),
                                };
                                let is_toolset = matches!(row, SelectionRow::Toolset { .. });
                                rsx! {
                                    SelectionItem {
                                        key: "{key}",
                                        label,
                                        detail,
                                        add_label: "Add tool".to_string(),
                                        add_aria: format!("Add tool {add_name}"),
                                        disabled: is_saving,
                                        on_add: move |_| {
                                            tools_stage.with_mut(|s| {
                                                if is_toolset {
                                                    enable_toolset(s, &add_name);
                                                } else {
                                                    enable_tool(s, &add_name);
                                                }
                                            });
                                            save_msg.set(None);
                                        },
                                    }
                                }
                            }
                        }
                    }
                    if tools_no_match {
                        div { class: "ih-slideover-note", "No tools match \"{tools_query_now}\"." }
                    }
                    if has_mcp {
                        div { class: "ih-slideover-note", "MCP servers are managed on the Tools screen." }
                    }
                }
                div { class: "ih-slideover-section",
                    label { class: "ih-slideover-label", "SKILLS" }
                    if let Some(ref reason) = skills_error {
                        div { class: "ih-slideover-error", role: "alert",
                            "Couldn't load skills: {reason}"
                            button {
                                r#type: "button",
                                class: "ih-btn-secondary",
                                onclick: move |_| {
                                    let cur = *skills_tick.peek();
                                    skills_tick.set(cur + 1);
                                },
                                "Retry now"
                            }
                        }
                    }
                    div { class: "ih-chip-box",
                        if !skills_ready && skills_error.is_none() {
                            div { class: "ih-slideover-note", "Loading skills\u{2026}" }
                        }
                        if skills_empty_catalog {
                            div { class: "ih-slideover-empty",
                                div { class: "ih-slideover-label", "No skills in the catalog" }
                                div { class: "ih-slideover-note",
                                    "Skills added to the root catalog show up here and can be turned off per agent."
                                }
                            }
                        }
                        if skills_ready && !skills_empty_catalog && skill_chip_names.is_empty() {
                            div { class: "ih-slideover-note", "All skills are turned off for {agent}." }
                        }
                        for skill_name in skill_chip_names.iter() {
                            {
                                let name = skill_name.clone();
                                let remove_name = skill_name.clone();
                                rsx! {
                                    Chip {
                                        key: "{name}",
                                        label: name.clone(),
                                        remove_label: format!("Disable {name}"),
                                        disabled: is_saving,
                                        on_remove: move |_| {
                                            skills_stage.with_mut(|s| {
                                                disable_skill(s, &remove_name);
                                            });
                                            save_msg.set(None);
                                        },
                                    }
                                }
                            }
                        }
                    }
                    input {
                        r#type: "search",
                        class: "ih-search",
                        placeholder: "Search skills",
                        "aria-label": "Search skills",
                        disabled: !skills_ready,
                        value: "{skills_query_now}",
                        oninput: move |e| skills_query.set(e.value()),
                    }
                    if !skill_off_all.is_empty() {
                        div { class: "ih-slideover-note", "Skill selection" }
                    }
                    div {
                        class: "ih-select-list",
                        role: "list",
                        "aria-label": "Skill selection",
                        for row in skill_off.iter() {
                            {
                                let name = row.name.clone();
                                let add_name = row.name.clone();
                                rsx! {
                                    SelectionItem {
                                        key: "{name}",
                                        label: name.clone(),
                                        detail: row.category.clone(),
                                        add_label: "Add skill".to_string(),
                                        add_aria: format!("Add skill {name}"),
                                        disabled: is_saving,
                                        on_add: move |_| {
                                            skills_stage.with_mut(|s| {
                                                enable_skill(s, &add_name);
                                            });
                                            save_msg.set(None);
                                        },
                                    }
                                }
                            }
                        }
                    }
                    if skills_no_match {
                        div { class: "ih-slideover-note", "No skills match \"{skills_query_now}\"." }
                    }
                }
                if let Some(ref text) = save_ok {
                    div { class: "ih-slideover-ok", role: "status", "{text}" }
                }
                if let Some(ref text) = save_err {
                    div { class: "ih-slideover-error", role: "alert", "{text}" }
                }
            }
            div { class: "ih-slideover-footer",
                div { class: "ih-slideover-footer-row",
                    button {
                        r#type: "button",
                        class: "ih-btn-secondary ih-load-default",
                        disabled: loading || is_saving,
                        onclick: move |_| {
                            // LOAD-DEFAULT-BEGIN (D-19): stage the root
                            // profile's values into the working copy. It
                            // reads the root config and writes NOTHING; only
                            // Save changes persists.
                            let page = defaults_page.clone();
                            let rows = defaults_rows.clone();
                            spawn(async move {
                                match fetch_root_agent_defaults().await {
                                    Ok(defaults) => {
                                        let current_tools = tools_stage.peek().clone();
                                        let current_skills = skills_stage.peek().clone();
                                        let applied = apply_root_defaults(
                                            &defaults,
                                            page.as_ref(),
                                            &current_tools,
                                            rows.as_deref(),
                                            &current_skills,
                                        );
                                        // An unset root provider/model has nothing
                                        // to load: keep what is staged (CR-01).
                                        let next_provider = keep_when_unset(
                                            &applied.provider,
                                            &provider_wc.peek().clone(),
                                        );
                                        let next_model = keep_when_unset(
                                            &applied.model,
                                            &model_wc.peek().clone(),
                                        );
                                        provider_wc.set(next_provider);
                                        model_wc.set(next_model);
                                        tools_stage.set(applied.tools);
                                        skills_stage.set(applied.skills);
                                        save_msg.set(None);
                                    }
                                    Err(e) => {
                                        save_msg
                                            .set(
                                                Some(
                                                    Err(
                                                        format!("Couldn't load the root defaults: {e}"),
                                                    ),
                                                ),
                                            );
                                    }
                                }
                            });
                            // LOAD-DEFAULT-END
                        },
                        "Load default"
                    }
                    button {
                        r#type: "button",
                        class: "ih-btn-secondary",
                        disabled: discard_disabled,
                        onclick: move |_| {
                            let (p, m) = seed.peek().clone();
                            provider_wc.set(p);
                            model_wc.set(m);
                            let staged = tools_seed.peek().clone();
                            tools_stage.set(staged);
                            let staged_skills = skills_seed.peek().clone();
                            skills_stage.set(staged_skills);
                            save_msg.set(None);
                        },
                        "Discard edits"
                    }
                    button {
                        r#type: "button",
                        class: "ih-btn-primary",
                        disabled: save_disabled,
                        onclick: move |_| {
                            let p = provider_wc.peek().clone();
                            let m = model_wc.peek().clone();
                            let seed_now = seed.peek().clone();
                            // A section whose data never loaded is sent as
                            // None, so Save cannot overwrite what it never saw.
                            let tools_snapshot = tools_stage.peek().clone();
                            let tools_part = if *tools_seeded.peek() {
                                tools_payload(&tools_snapshot, &tools_seed.peek().clone())
                            } else {
                                (None, None, None)
                            };
                            let skills_snapshot = skills_stage.peek().clone();
                            let skills_part = if *skills_seeded.peek() {
                                skills_payload(&skills_snapshot, &skills_seed.peek().clone())
                            } else {
                                None
                            };
                            let payload =
                                build_save_payload(&save_agent, &p, &m, &seed_now, tools_part, skills_part);
                            let name = save_agent.clone();
                            let mut refresh_tick = refresh;
                            saving.set(true);
                            save_msg.set(None);
                            spawn(async move {
                                match update_profile_config(payload).await {
                                    Ok(()) => {
                                        seed.set((p, m));
                                        tools_seed.set(tools_snapshot);
                                        skills_seed.set(skills_snapshot);
                                        save_msg.set(Some(Ok(save_success_copy(&name))));
                                        // WR-04: the header's PROVIDER / MODEL / ENV
                                        // and the agent dropdown's meta line read
                                        // this tick; without the bump they keep the
                                        // pre-save values until the next refocus.
                                        let cur = *refresh_tick.peek();
                                        refresh_tick.set(cur + 1);
                                    }
                                    Err(e) => {
                                        save_msg
                                            .set(Some(Err(save_error_copy(&name, &format!("{e}")))));
                                    }
                                }
                                saving.set(false);
                            });
                        },
                        if is_saving {
                            "Saving\u{2026}"
                        } else {
                            "Save changes"
                        }
                    }
                }
                div { class: "ih-slideover-helper",
                    "Copies provider, model, disabled skills and disabled tools from the root profile. Nothing is saved until you press Save changes."
                }
                div { class: "ih-slideover-footer-row",
                    button {
                        r#type: "button",
                        class: "ih-btn-danger",
                        onclick: move |_| {
                            // Dirty guard: unsaved edits ask first; a clean
                            // close removes the tab and hides the panel, and
                            // records the focus it moved to (WR-02).
                            update_tabs_recording_focus(tabs, notice, refresh, |s| {
                                s.request_close(&close_agent)
                            });
                        },
                        "Close agent"
                    }
                    span { class: "ih-slideover-helper",
                        "Closes this agent's tab. The profile and its files are kept."
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::hermes_app::agent_tabs::{
        AgentNoticeCtx, AgentTabsState, LiveAgentCtx, PanelState,
    };
    use crate::components::hermes_app::profile_topbar::TopbarProfileRefreshCtx;
    use crate::state::Screen;

    fn pair(a: &str, b: &str) -> (String, String) {
        (a.to_string(), b.to_string())
    }

    #[test]
    fn agent_settings_payload_carries_changed_provider_and_model_only() {
        let seed = pair("openrouter", "old-model");
        let p = build_save_payload(
            "dev",
            "anthropic",
            "claude-x",
            &seed,
            (None, None, None),
            None,
        );
        assert_eq!(p.name, "dev");
        assert_eq!(p.provider.as_deref(), Some("anthropic"));
        assert_eq!(p.model_default.as_deref(), Some("claude-x"));
        assert!(p.skills_disabled.is_none());
        assert!(p.tools_toolsets.is_none());
        assert!(p.tools_disable.is_none());
        assert!(p.tools_enable.is_none());
        // Unchanged fields are not part of the edit.
        let same = build_save_payload(
            "dev",
            "openrouter",
            "claude-x",
            &seed,
            (None, None, None),
            None,
        );
        assert_eq!(same.provider, None);
        assert_eq!(same.model_default.as_deref(), Some("claude-x"));
    }

    /// CR-01: a profile that inherits both provider and model (blank seed)
    /// must be able to save a tools / skills edit; the payload must never carry
    /// `Some("")`, which the server rejects.
    #[test]
    fn agent_settings_unset_provider_and_model_still_save_tools_and_skills() {
        let blank = pair("", "");
        let p = build_save_payload(
            "dev",
            "",
            "",
            &blank,
            (None, Some(vec!["web_extract".to_string()]), None),
            Some(vec!["old-skill".to_string()]),
        );
        assert_eq!(p.provider, None);
        assert_eq!(p.model_default, None);
        assert_eq!(p.tools_disable, Some(vec!["web_extract".to_string()]));
        assert_eq!(p.skills_disabled, Some(vec!["old-skill".to_string()]));
        // Whitespace-only is still "inherit", never an explicit empty string.
        let ws = build_save_payload("dev", "  ", "\t", &blank, (None, None, None), None);
        assert_eq!((ws.provider, ws.model_default), (None, None));
        // Clearing a pinned value back to blank is not expressible as an
        // empty string either: it sends None, not `Some("")`.
        let cleared = build_save_payload("dev", "", "", &pair("a", "b"), (None, None, None), None);
        assert_eq!((cleared.provider, cleared.model_default), (None, None));
    }

    /// CR-01: "Load default" from a root with no provider/model keeps the
    /// staged values instead of staging empty strings, and the Save that
    /// follows carries no empty provider/model.
    #[test]
    fn agent_settings_load_default_from_unset_root_saves_cleanly() {
        let root = crate::protocol::RootAgentDefaults {
            provider: String::new(),
            model: "  ".to_string(),
            skills_disabled: vec![],
            disabled_toolsets: vec![],
            tools_disabled: vec![],
        };
        let applied = apply_root_defaults(
            &root,
            None,
            &ToolsStage::default(),
            None,
            &SkillsStage::default(),
        );
        let provider = keep_when_unset(&applied.provider, "openrouter");
        let model = keep_when_unset(&applied.model, "");
        assert_eq!(provider, "openrouter");
        assert_eq!(model, "");
        let seed = pair("openrouter", "");
        let payload = build_save_payload(
            "dev",
            &provider,
            &model,
            &seed,
            (None, Some(vec!["web_extract".to_string()]), None),
            None,
        );
        assert_eq!(payload.provider, None);
        assert_eq!(payload.model_default, None);
        assert_eq!(payload.tools_disable, Some(vec!["web_extract".to_string()]));
        // A set root value is loaded as before.
        assert_eq!(keep_when_unset("p", "old"), "p");
    }

    #[test]
    fn agent_settings_dirty_when_working_copy_differs() {
        assert!(!is_dirty(&pair("a", "m"), &pair("a", "m")));
        assert!(is_dirty(&pair("a", "m"), &pair("a", "m2")));
        assert!(is_dirty(&pair("a", "m"), &pair("b", "m")));
    }

    #[test]
    fn agent_settings_provider_choices_keep_a_stale_assigned_provider() {
        let configured = vec!["openrouter".to_string()];
        assert_eq!(
            provider_choices(&configured, "old-provider"),
            vec!["old-provider".to_string(), "openrouter".to_string()]
        );
        assert_eq!(provider_choices(&configured, "openrouter"), configured);
        assert_eq!(provider_choices(&configured, "  "), configured);
    }

    #[test]
    fn agent_settings_copy_matches_the_ui_spec() {
        assert_eq!(
            save_error_copy("dev", "disk full."),
            "Couldn't save dev's settings: disk full. Your edits are still here. Fix the problem and press Save changes again."
        );
        assert_eq!(
            save_success_copy("dev"),
            "Saved. Applies to dev's next launch."
        );
        assert_eq!(
            scope_note("dev"),
            "Saved changes apply the next time dev launches. The running chat is not affected."
        );
    }

    #[test]
    fn agent_settings_confirm_copy() {
        assert_eq!(
            confirm_copy(&PanelPending::Close("dev".into()), "dev"),
            (
                "Close agent: Close dev? You have unsaved settings changes that will be lost."
                    .to_string(),
                "Close agent"
            )
        );
        let discard = (
            "Discard changes: Discard unsaved changes to dev?".to_string(),
            "Discard changes",
        );
        assert_eq!(
            confirm_copy(&PanelPending::Focus("ops".into()), "dev"),
            discard
        );
        assert_eq!(confirm_copy(&PanelPending::Hide, "dev"), discard);
        assert_eq!(
            confirm_copy(&PanelPending::Navigate(Screen::Kanban), "dev"),
            discard
        );
    }

    const AGENT_SETTINGS_RS: &str = include_str!("agent_settings.rs");
    const DASHBOARD_CSS: &str = include_str!("../../../../../assets/dashboard.css");

    /// Source text before the first `#[cfg(test)]`, so this module's own
    /// needles can never satisfy (or trip) a check on the component code.
    fn code_of(src: &str) -> &str {
        &src[..src.find("#[cfg(test)]").unwrap_or(src.len())]
    }

    fn root_defaults() -> crate::protocol::RootAgentDefaults {
        crate::protocol::RootAgentDefaults {
            provider: "p".to_string(),
            model: "m".to_string(),
            skills_disabled: vec!["a".to_string(), "b".to_string()],
            disabled_toolsets: vec!["web".to_string()],
            tools_disabled: vec!["x".to_string(), "y".to_string(), "z".to_string()],
        }
    }

    #[test]
    fn staging_load_default_writes_nothing() {
        let code = code_of(AGENT_SETTINGS_RS);
        // Needles are assembled so this test can never satisfy itself.
        let begin = ["LOAD-DEFAULT", "-BEGIN"].concat();
        let end = ["LOAD-DEFAULT", "-END"].concat();
        let start = code
            .find(&begin)
            .expect("the Load default handler must be marked");
        let stop = start
            + code[start..]
                .find(&end)
                .expect("the Load default handler must be closed");
        let region = &code[start..stop];
        for needle in ["fetch_root_agent_defaults(", "apply_root_defaults("] {
            assert!(region.contains(needle), "Load default must call `{needle}`");
        }
        let write = ["update_profile", "_config("].concat();
        assert!(
            !region.contains(&write),
            "Load default stages values in the working copy; it must not write"
        );
        // The one write in this module is Save's.
        assert_eq!(
            code.matches(&write).count(),
            1,
            "only Save changes may call the write server fn"
        );
    }

    #[test]
    fn agent_settings_default_agent_is_read_only() {
        assert_eq!(panel_mode("default"), PanelMode::ReadOnlyRoot);
        assert_eq!(panel_mode("dev"), PanelMode::Editable);
        let code = code_of(AGENT_SETTINGS_RS);
        let start = code
            .find("fn RootAgentPanel")
            .expect("the read-only root branch must exist");
        let region = &code[start..];
        let region = &region[..region.find("\n#[component]").unwrap_or(region.len())];
        for needle in ["Open Providers", "Open Models", "Open Tools", "ROOT_NOTE"] {
            assert!(region.contains(needle), "root panel is missing `{needle}`");
        }
        let note =
            "This is the root profile. Change its provider, model and tools on their own screens.";
        assert!(
            code.contains(note),
            "the root note copy must match the UI-SPEC"
        );
        // D-22: no write call, no staged-edit controls in the root branch.
        let forbidden = [
            ["update_profile", "_config("].concat(),
            ["Save ", "changes"].concat(),
            ["Discard ", "edits"].concat(),
            ["Load ", "default"].concat(),
            ["Apply", "ConfigPendingCtx"].concat(),
        ];
        for needle in &forbidden {
            assert!(
                !region.contains(needle.as_str()),
                "root panel must not contain `{needle}`"
            );
        }
        // And nowhere in this module: a profile save never raises APPLY NOW.
        assert!(!code.contains(&forbidden[4]));
    }

    #[test]
    fn agent_settings_root_rows_show_counts() {
        assert_eq!(
            root_rows(&root_defaults()),
            vec![
                ("Provider", "p".to_string()),
                ("Model", "m".to_string()),
                ("Disabled skills", "2".to_string()),
                ("Disabled tools", "4".to_string()),
            ]
        );
        let empty = crate::protocol::RootAgentDefaults::default();
        let rows = root_rows(&empty);
        assert_eq!(rows[0].1, "\u{2014}", "an unset provider reads as a dash");
        assert_eq!(rows[2].1, "0");
    }

    #[test]
    fn agent_settings_save_enabled_rules() {
        assert!(save_enabled(true, true, false, false));
        assert!(!save_enabled(false, true, false, false), "needs edits");
        assert!(
            !save_enabled(true, false, false, false),
            "needs the write gate"
        );
        assert!(!save_enabled(true, true, true, false), "not while saving");
        assert!(!save_enabled(true, true, false, true), "not while loading");
    }

    /// The first `{ ... }` body following `selector` in comment-free CSS.
    fn rule_body<'a>(css: &'a str, selector: &str) -> &'a str {
        let at = css
            .find(selector)
            .unwrap_or_else(|| panic!("{selector} rule missing"));
        let open = at + css[at..].find('{').expect("rule has a body");
        let close = open + css[open..].find('}').expect("rule is closed");
        &css[open + 1..close]
    }

    #[test]
    fn dashboard_css_slideover_never_covers_the_wheel() {
        // Drop /* ... */ comments so prose can neither satisfy nor trip a check.
        let mut css = String::new();
        let mut rest = DASHBOARD_CSS;
        while let Some(start) = rest.find("/*") {
            css.push_str(&rest[..start]);
            match rest[start + 2..].find("*/") {
                Some(end) => rest = &rest[start + 2 + end + 2..],
                None => rest = "",
            }
        }
        css.push_str(rest);

        let body = rule_body(&css, "\n.ih-slideover {");
        for needle in [
            "min(var(--ih-slideover-w), calc(100vw - var(--ih-wheel-size)))",
            "top: var(--ih-header-h)",
            "z-index: 60",
            "position: fixed",
        ] {
            assert!(body.contains(needle), ".ih-slideover is missing `{needle}`");
        }
        // The reduced-motion @media block (closed by a line-start `}`) that
        // mentions .ih-slideover.
        let media = css
            .match_indices("@media (prefers-reduced-motion: reduce)")
            .map(|(i, _)| {
                let tail = &css[i..];
                &tail[..tail.find("\n}").map_or(tail.len(), |e| e + 2)]
            })
            .find(|block| block.contains(".ih-slideover"))
            .expect("a reduced-motion block must cover .ih-slideover");
        let motion = rule_body(media, ".ih-slideover");
        assert!(motion.contains("transition: none"));
        assert!(motion.contains("animation: none"));
    }

    #[test]
    fn smoke_agent_settings_mounts_the_root_panel() {
        fn test_app() -> Element {
            let screen = use_signal(|| Screen::Chat);
            use_context_provider(|| screen);
            let tabs = use_signal(|| AgentTabsState {
                open: vec!["default".to_string()],
                focused: Some("default".to_string()),
                ..Default::default()
            });
            use_context_provider(|| AgentTabsCtx(tabs));
            let live =
                use_resource(|| async { Ok::<String, ServerFnError>("default".to_string()) });
            use_context_provider(|| LiveAgentCtx(live));
            let notice = use_signal(|| None::<String>);
            use_context_provider(|| AgentNoticeCtx(notice));
            let tick = use_signal(|| 0_u32);
            use_context_provider(|| TopbarProfileRefreshCtx(tick));
            rsx! { AgentSettingsPanel { agent: "default".to_string() } }
        }

        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_to_vec();
    }

    #[test]
    fn smoke_agent_settings_mounts_with_a_pending_confirmation() {
        fn test_app() -> Element {
            let screen = use_signal(|| Screen::Chat);
            use_context_provider(|| screen);
            let tabs = use_signal(|| AgentTabsState {
                open: vec!["default".to_string(), "dev".to_string()],
                focused: Some("dev".to_string()),
                panel: PanelState {
                    settings_for: Some("dev".to_string()),
                    dirty: true,
                    pending: Some(PanelPending::Navigate(Screen::Kanban)),
                },
                ..Default::default()
            });
            use_context_provider(|| AgentTabsCtx(tabs));
            let live =
                use_resource(|| async { Ok::<String, ServerFnError>("default".to_string()) });
            use_context_provider(|| LiveAgentCtx(live));
            let notice = use_signal(|| None::<String>);
            use_context_provider(|| AgentNoticeCtx(notice));
            let tick = use_signal(|| 0_u32);
            use_context_provider(|| TopbarProfileRefreshCtx(tick));
            rsx! { AgentSettingsPanel { agent: "dev".to_string() } }
        }

        let mut dom = VirtualDom::new(test_app);
        // The confirm-bar branch (an `if let` over owned locals) must render
        // without a hook-order or borrow panic.
        dom.rebuild_to_vec();
    }

    #[test]
    fn smoke_agent_settings_mounts_without_panic() {
        fn test_app() -> Element {
            let screen = use_signal(|| Screen::Chat);
            use_context_provider(|| screen);
            let tabs = use_signal(|| AgentTabsState {
                open: vec!["default".to_string(), "dev".to_string()],
                focused: Some("dev".to_string()),
                ..Default::default()
            });
            use_context_provider(|| AgentTabsCtx(tabs));
            let live =
                use_resource(|| async { Ok::<String, ServerFnError>("default".to_string()) });
            use_context_provider(|| LiveAgentCtx(live));
            let notice = use_signal(|| None::<String>);
            use_context_provider(|| AgentNoticeCtx(notice));
            let tick = use_signal(|| 0_u32);
            use_context_provider(|| TopbarProfileRefreshCtx(tick));
            rsx! { AgentSettingsPanel { agent: "dev".to_string() } }
        }

        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_to_vec();
    }

    /// WR-04: a successful Save bumps the shared profile-refresh tick, which
    /// the header (PROVIDER / MODEL / ENV) and the agent dropdown's meta line
    /// read; the bump sits in the `Ok(())` arm only.
    #[test]
    fn agent_settings_save_bumps_the_refresh_tick_on_success_only() {
        let code = code_of(AGENT_SETTINGS_RS);
        let save = code
            .find("match update_profile_config(payload).await {")
            .expect("the Save handler must call update_profile_config");
        let region = &code[save..];
        let ok_at = region.find("Ok(()) =>").expect("Save must handle success");
        let err_at = region.find("Err(e) =>").expect("Save must handle failure");
        let bump_at = region
            .find("refresh_tick.set(")
            .expect("a successful Save must bump the refresh tick");
        assert!(
            ok_at < bump_at && bump_at < err_at,
            "the bump belongs to the Ok arm, never the Err arm"
        );
    }

    /// `dashboard.css` with `/* ... */` comments removed, so prose can neither
    /// satisfy nor trip a check.
    fn css_without_comments() -> String {
        let mut css = String::new();
        let mut rest = DASHBOARD_CSS;
        while let Some(start) = rest.find("/*") {
            css.push_str(&rest[..start]);
            match rest[start + 2..].find("*/") {
                Some(end) => rest = &rest[start + 2 + end + 2..],
                None => rest = "",
            }
        }
        css.push_str(rest);
        css
    }

    thread_local! {
        /// The pending action the SSR test app mounts with. A `fn() -> Element`
        /// cannot capture, so the value travels through a thread-local.
        static TEST_PENDING: std::cell::RefCell<Option<PanelPending>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Render the REAL `AgentSettingsPanel` for "dev" (dirty, with `pending`
    /// parked) to HTML. G-54.4-5 shipped past a source-text smoke test, so the
    /// element order is asserted on the rendered output instead.
    fn render_panel_html(pending: Option<PanelPending>) -> String {
        TEST_PENDING.with(|p| *p.borrow_mut() = pending);

        fn test_app() -> Element {
            let screen = use_signal(|| Screen::Chat);
            use_context_provider(|| screen);
            let tabs = use_signal(|| AgentTabsState {
                open: vec!["default".to_string(), "dev".to_string()],
                focused: Some("dev".to_string()),
                panel: PanelState {
                    settings_for: Some("dev".to_string()),
                    dirty: true,
                    pending: TEST_PENDING.with(|p| p.borrow().clone()),
                },
                ..Default::default()
            });
            use_context_provider(|| AgentTabsCtx(tabs));
            let live =
                use_resource(|| async { Ok::<String, ServerFnError>("default".to_string()) });
            use_context_provider(|| LiveAgentCtx(live));
            let notice = use_signal(|| None::<String>);
            use_context_provider(|| AgentNoticeCtx(notice));
            let tick = use_signal(|| 0_u32);
            use_context_provider(|| TopbarProfileRefreshCtx(tick));
            rsx! { AgentSettingsPanel { agent: "dev".to_string() } }
        }

        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    /// G-54.4-5 / D-16: for every parked action the confirmation bar is a
    /// sibling BETWEEN the title bar and the scrolling body, never inside the
    /// body, so the body's scrollTop and scroll anchoring cannot carry it off
    /// screen.
    #[test]
    fn confirm_bar_renders_outside_the_scrolling_body() {
        let kinds = [
            PanelPending::Navigate(Screen::Kanban),
            PanelPending::Hide,
            PanelPending::Focus("default".to_string()),
            PanelPending::Close("dev".to_string()),
        ];
        for pending in kinds {
            let html = render_panel_html(Some(pending.clone()));
            let aside = html.find("class=\"ih-slideover\"");
            let bar = html.find("class=\"ih-confirm-bar\"");
            let body = html.find("class=\"ih-slideover-body\"");
            let offsets = format!("aside={aside:?} bar={bar:?} body={body:?}");
            let (Some(aside), Some(bar), Some(body)) = (aside, bar, body) else {
                panic!("{pending:?}: the panel, bar and body must all render ({offsets})");
            };
            assert!(
                aside < bar && bar < body,
                "{pending:?}: the bar must sit after the aside opens and BEFORE the scrolling body ({offsets})"
            );
            assert_eq!(
                html.matches("class=\"ih-confirm-bar\"").count(),
                1,
                "{pending:?}: exactly one confirm bar ({offsets})"
            );
            let question = confirm_copy(&pending, "dev").0;
            assert!(
                html.contains(&question),
                "{pending:?}: the UI-SPEC question `{question}` must render ({offsets})"
            );
        }
    }

    #[test]
    fn no_confirm_bar_without_a_pending_action() {
        let html = render_panel_html(None);
        assert!(
            html.contains("class=\"ih-slideover-body\""),
            "the panel body must render"
        );
        assert_eq!(
            html.matches("ih-confirm-bar").count(),
            0,
            "no confirm bar without a parked action"
        );
    }

    #[test]
    fn keep_editing_restores_chat_only_for_navigate() {
        assert_eq!(
            keep_editing_screen(Some(&PanelPending::Navigate(Screen::Kanban))),
            Some(Screen::Chat)
        );
        assert_eq!(keep_editing_screen(Some(&PanelPending::Hide)), None);
        assert_eq!(
            keep_editing_screen(Some(&PanelPending::Focus("default".to_string()))),
            None
        );
        assert_eq!(
            keep_editing_screen(Some(&PanelPending::Close("dev".to_string()))),
            None
        );
        assert_eq!(keep_editing_screen(None), None);
    }

    thread_local! {
        /// (parked action, screen the router currently shows) for the Esc test.
        static ESC_SETUP: std::cell::RefCell<Option<(PanelPending, Screen)>> =
            const { std::cell::RefCell::new(None) };
        /// Copies of the app's signals, read back after the key press.
        static ESC_SIGNALS: std::cell::RefCell<Option<(Signal<AgentTabsState>, Signal<Screen>)>> =
            const { std::cell::RefCell::new(None) };
        /// Whether the ancestor of the bar saw the Esc keydown bubble up.
        static ESC_PARENT_SAW_KEY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// The bar root's ElementId in `esc_app`: ElementId(0) is the VirtualDom
    /// root, the wrapper div is the first template instantiated (ElementId(1),
    /// observed as the only `keydown` listener before the bar had one) and the
    /// bar's root `div` is the second. Pinned, and cross-checked against the
    /// `keydown` listeners the initial render registers, so a markup change
    /// fails an assertion instead of delivering the key to the wrong node.
    const ESC_BAR_ID: usize = 2;

    fn esc_app() -> Element {
        let (pending, start) = ESC_SETUP
            .with(|s| s.borrow().clone())
            .expect("ESC_SETUP must be set before the dom is built");
        let screen = use_signal(move || start);
        use_context_provider(|| screen);
        let seeded = pending.clone();
        let tabs = use_signal(move || AgentTabsState {
            open: vec!["default".to_string(), "dev".to_string()],
            focused: Some("dev".to_string()),
            panel: PanelState {
                settings_for: Some("dev".to_string()),
                dirty: true,
                pending: Some(seeded),
            },
            ..Default::default()
        });
        use_context_provider(|| AgentTabsCtx(tabs));
        let notice = use_signal(|| None::<String>);
        use_context_provider(|| AgentNoticeCtx(notice));
        let tick = use_signal(|| 0_u32);
        use_context_provider(|| TopbarProfileRefreshCtx(tick));
        ESC_SIGNALS.with(|s| *s.borrow_mut() = Some((tabs, screen)));
        rsx! {
            div { onkeydown: move |_| ESC_PARENT_SAW_KEY.with(|c| c.set(true)),
                ConfirmBar { agent: "dev".to_string(), pending }
            }
        }
    }

    /// Mount the bar with `pending` parked, press Esc on it, and return the
    /// panel state and screen afterwards plus whether the ancestor saw the key.
    fn press_escape_in_confirm_bar(pending: PanelPending, shown: Screen) -> (PanelState, Screen, bool) {
        use dioxus::dioxus_core::{ElementId, Mutation};
        use dioxus::html::{Code, Location, Modifiers, PlatformEventData, SerializedKeyboardData};
        use std::any::Any;
        use std::rc::Rc;

        ESC_SETUP.with(|s| *s.borrow_mut() = Some((pending, shown)));
        ESC_PARENT_SAW_KEY.with(|c| c.set(false));
        dioxus::html::set_event_converter(Box::new(dioxus::html::SerializedHtmlEventConverter));

        let mut dom = VirtualDom::new(esc_app);
        let initial = dom.rebuild_to_vec();
        let keydown_ids: Vec<ElementId> = initial
            .edits
            .iter()
            .filter_map(|m| match m {
                Mutation::NewEventListener { name, id } if *name == "keydown" => Some(*id),
                _ => None,
            })
            .collect();
        assert!(
            keydown_ids.contains(&ElementId(ESC_BAR_ID)),
            "ElementId({ESC_BAR_ID}) must carry a keydown listener; the render registered {keydown_ids:?}"
        );

        let escape = Event::new(
            Rc::new(PlatformEventData::new(Box::new(SerializedKeyboardData::new(
                Key::Escape,
                Code::Escape,
                Location::Standard,
                false,
                Modifiers::empty(),
                false,
            )))) as Rc<dyn Any>,
            true,
        );
        dom.runtime()
            .handle_event("keydown", escape, ElementId(ESC_BAR_ID));
        dom.render_immediate_to_vec();

        let (panel, screen) = dom.in_runtime(|| {
            ESC_SIGNALS.with(|s| {
                let (tabs, screen) = s.borrow().expect("the app registered its signals");
                let panel = tabs.peek().panel.clone();
                let shown = *screen.peek();
                (panel, shown)
            })
        });
        (panel, screen, ESC_PARENT_SAW_KEY.with(|c| c.get()))
    }

    /// G-54.4-5: Esc inside the bar is Keep editing. It stops propagation, so
    /// the panel's own Esc handler (`request_hide`) cannot re-park a Hide.
    #[test]
    fn confirm_bar_escape_keeps_editing() {
        // (1) A held navigation: Chat comes back, the panel stays open and dirty.
        let (panel, screen, parent_saw) =
            press_escape_in_confirm_bar(PanelPending::Navigate(Screen::Kanban), Screen::Kanban);
        assert_eq!(panel.pending, None, "Esc clears the pending action");
        assert_eq!(panel.settings_for.as_deref(), Some("dev"), "panel stays open");
        assert!(panel.dirty, "the dirty working copy is kept");
        assert_eq!(screen, Screen::Chat, "a held navigation puts Chat back");
        assert!(!parent_saw, "Esc must not bubble to the aside's own handler");

        // (2) A parked Hide: cleared, and NOT hidden or re-parked.
        let (panel, screen, parent_saw) =
            press_escape_in_confirm_bar(PanelPending::Hide, Screen::Chat);
        assert_eq!(panel.pending, None, "Esc clears a parked Hide");
        assert_eq!(
            panel.settings_for.as_deref(),
            Some("dev"),
            "the panel was NOT hidden"
        );
        assert!(panel.dirty, "the dirty working copy is kept");
        assert_eq!(screen, Screen::Chat);
        assert!(!parent_saw, "Esc must not bubble to the aside's own handler");
    }

    /// The bar takes focus on its SAFE action. `onmounted` does not fire
    /// natively, so this pins the wiring; the landing is the human check.
    #[test]
    fn confirm_bar_focuses_keep_editing_on_mount() {
        let html = render_panel_html(Some(PanelPending::Navigate(Screen::Kanban)));
        let at = html
            .find("id=\"ih-confirm-keep\"")
            .expect("the Keep editing button must carry id=ih-confirm-keep");
        let tag_start = html[..at].rfind("<button").expect("the id sits on a <button>");
        assert!(
            !html[tag_start..at].contains('>'),
            "id=ih-confirm-keep must be an attribute of the nearest <button> tag"
        );
        let open_end = at + html[at..].find('>').expect("the tag closes");
        let close = open_end + html[open_end..].find("</button>").expect("button closes");
        assert_eq!(&html[open_end + 1..close], "Keep editing");

        let code = code_of(AGENT_SETTINGS_RS);
        let start = code.find("fn ConfirmBar(").expect("ConfirmBar must exist");
        let region = &code[start..];
        let region = &region[..region.find("#[component]").unwrap_or(region.len())];
        assert_eq!(region.matches("onmounted").count(), 1, "one onmounted");
        assert_eq!(region.matches("set_focus(true)").count(), 1, "one set_focus");
        let danger = region
            .find("ih-btn-danger")
            .expect("the destructive button exists");
        let secondary = region
            .find("ih-btn-secondary")
            .expect("the Keep editing button exists");
        assert!(danger < secondary, "Discard precedes Keep editing (UI-SPEC)");
        let danger_block = &region[danger..secondary];
        assert!(
            !danger_block.contains("onmounted") && !danger_block.contains("set_focus"),
            "the destructive button must never take focus automatically"
        );

        let at = code.find("fn keep_editing(").expect("keep_editing must exist");
        let helper = &code[at..];
        let helper = &helper[..helper.find("\n}\n").map_or(helper.len(), |e| e + 3)];
        let needle = "focus_element_by_id(\"ih-slideover-title\")";
        assert_eq!(helper.matches(needle).count(), 1, "focus returns to the title");
        assert!(
            helper.find("cancel_pending").expect("cancel_pending is called")
                < helper.find(needle).expect("title focus"),
            "focus returns AFTER the pending action is cleared"
        );
    }

    #[test]
    fn dashboard_css_confirm_bar_is_pinned() {
        let css = css_without_comments();
        let bar = rule_body(&css, "\n.ih-confirm-bar {");
        assert!(
            bar.contains("flex: 0 0 auto"),
            ".ih-confirm-bar must never shrink in the flex-column aside"
        );
        let body = rule_body(&css, "\n.ih-slideover-body {");
        assert!(
            body.contains("overflow-y: auto"),
            ".ih-slideover-body stays the panel's scroller"
        );
    }
}

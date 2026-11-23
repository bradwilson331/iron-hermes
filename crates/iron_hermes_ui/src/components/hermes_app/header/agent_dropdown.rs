//! Phase 54.4 Plan 05 (D-12, D-22, D-23) — the header's Agent dropdown.
//!
//! The first control in header row 2. It replaces the retired
//! `TopbarProfileSwitch` (49.4 D-19: never two profile switchers) and reuses
//! its data path: the profile list comes from the root-provided
//! `SharedProfilesCtx` only, never a second `list_profiles` call.
//!
//! Picking a row calls [`focus_agent`] (focus or open exactly one tab for the
//! agent, then a best-effort activation write), navigates to `Screen::Chat`
//! from any other screen (Interaction Rule 7), closes the menu and returns
//! focus to the trigger. The root agent is always the first row (D-22), so the
//! menu is never empty.
//!
//! This is a standalone component, never an inline `<select>` (the Soul
//! dropdown revert 073d4e671 rendered blank under a green gate). It declares
//! no context provider; it only consumes the contexts `HermesApp` provides.

use crate::components::hermes_app::agent_tabs::{
    focus_agent, live_agent_name, AgentNoticeCtx, AgentTabsCtx, AgentTabsState, LiveAgentCtx,
    DEFAULT_AGENT,
};
use crate::components::hermes_app::header::{focus_element_by_id, menu_next_index, MenuKey};
use crate::components::hermes_app::profile_topbar::{SharedProfilesCtx, TopbarProfileRefreshCtx};
use crate::components::hermes_app::screens::profile_shared::switcher::{
    profile_health_is_ok, profile_meta_line,
};
use crate::protocol::ProfileRow;
use crate::state::Screen;
use dioxus::prelude::*;

/// Meta line of the pinned root row.
const DEFAULT_ROW_META: &str = "Root home \u{2014} no profile override";

/// One rendered menu row.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct DropdownRow {
    pub name: String,
    pub meta: String,
    pub health_ok: bool,
    pub is_live: bool,
    pub is_open: bool,
}

/// The menu rows: the root agent first, then the profiles alphabetically.
/// `live` is the live agent's name; `open` the open tab ids.
pub(crate) fn dropdown_rows(
    profiles: &[ProfileRow],
    live: Option<&str>,
    open: &[String],
) -> Vec<DropdownRow> {
    let is_open = |name: &str| open.iter().any(|o| o == name);
    let mut rows = vec![DropdownRow {
        name: DEFAULT_AGENT.to_string(),
        meta: DEFAULT_ROW_META.to_string(),
        health_ok: true,
        is_live: live == Some(DEFAULT_AGENT),
        is_open: is_open(DEFAULT_AGENT),
    }];
    let mut sorted: Vec<&ProfileRow> = profiles
        .iter()
        .filter(|p| p.name != DEFAULT_AGENT)
        .collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    rows.extend(sorted.into_iter().map(|p| DropdownRow {
        name: p.name.clone(),
        meta: profile_meta_line(p),
        health_ok: profile_health_is_ok(&p.health),
        is_live: live == Some(p.name.as_str()),
        is_open: is_open(&p.name),
    }));
    rows
}

/// Pick an agent: focus/open its tab, show Chat, close the menu and return
/// focus to the trigger.
fn pick_agent(
    name: String,
    tabs: Signal<AgentTabsState>,
    notice: Signal<Option<String>>,
    refresh: Signal<u32>,
    mut active_screen: Signal<Screen>,
    mut open: Signal<bool>,
) {
    focus_agent(tabs, notice, refresh, name);
    active_screen.set(Screen::Chat);
    open.set(false);
    focus_element_by_id("ih-agent-trigger");
}

#[component]
pub fn AgentDropdown() -> Element {
    let tabs_ctx = use_context::<AgentTabsCtx>();
    let notice = use_context::<AgentNoticeCtx>().0;
    let live_ctx = use_context::<LiveAgentCtx>();
    let refresh = use_context::<TopbarProfileRefreshCtx>().0;
    let profiles_res = use_context::<SharedProfilesCtx>().0;
    let active_screen = use_context::<Signal<Screen>>();
    let mut open = use_signal(|| false);

    // After the menu mounts, move focus to its first row. Elements exist by
    // the time an effect runs.
    use_effect(move || {
        if *open.read() {
            focus_element_by_id("ih-agent-opt-0");
        }
    });

    // Owned locals BEFORE rsx! so no signal borrow is held across the tree.
    let tabs = tabs_ctx.0;
    let (focused, open_ids) = {
        let state = tabs.read();
        (
            state
                .focused
                .clone()
                .unwrap_or_else(|| DEFAULT_AGENT.to_string()),
            state.open.clone(),
        )
    };
    let live = live_agent_name(live_ctx.0());
    let (profiles, loading, load_error): (Vec<ProfileRow>, bool, Option<String>) =
        match profiles_res() {
            None => (Vec::new(), true, None),
            Some(Ok(rows)) => (rows, false, None),
            Some(Err(e)) => (Vec::new(), false, Some(e.to_string())),
        };
    let rows = dropdown_rows(&profiles, live.as_deref(), &open_ids);
    let row_count = rows.len();
    let is_open = *open.read();
    let expanded = if is_open { "true" } else { "false" };

    rsx! {
        div { class: "ih-agent-wrap",
            button {
                id: "ih-agent-trigger",
                class: "ih-agent-trigger",
                r#type: "button",
                "aria-haspopup": "listbox",
                "aria-expanded": "{expanded}",
                onclick: move |_| {
                    let now = *open.read();
                    open.set(!now);
                },
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::ArrowDown {
                        e.prevent_default();
                        open.set(true);
                    }
                },
                span { class: "ih-seg-label", "AGENT" }
                span { class: "ih-agent-name", title: "{focused}", "{focused}" }
                span { "aria-hidden": "true", "\u{25BE}" }
            }
            if is_open {
                div {
                    class: "ih-menu-backdrop",
                    onclick: move |_| open.set(false),
                }
                div { class: "ih-agent-menu", role: "listbox", "aria-label": "Agents",
                    for (i, row) in rows.iter().cloned().enumerate() {
                        {
                            let name_for_click = row.name.clone();
                            let name_for_key = row.name.clone();
                            let selected = if row.name == focused { "true" } else { "false" };
                            let dot_class = if row.health_ok {
                                "ih-agent-dot ih-agent-dot--ok"
                            } else {
                                "ih-agent-dot ih-agent-dot--gap"
                            };
                            rsx! {
                                div {
                                    key: "{row.name}",
                                    id: "ih-agent-opt-{i}",
                                    class: "ih-agent-opt",
                                    role: "option",
                                    "aria-selected": "{selected}",
                                    tabindex: "-1",
                                    title: "{row.name}",
                                    onclick: move |_| {
                                        pick_agent(
                                            name_for_click.clone(),
                                            tabs,
                                            notice,
                                            refresh,
                                            active_screen,
                                            open,
                                        );
                                    },
                                    onkeydown: move |e: KeyboardEvent| {
                                        let nav = match e.key() {
                                            Key::ArrowDown => Some(MenuKey::Down),
                                            Key::ArrowUp => Some(MenuKey::Up),
                                            Key::Home => Some(MenuKey::Home),
                                            Key::End => Some(MenuKey::End),
                                            _ => None,
                                        };
                                        if let Some(k) = nav {
                                            e.prevent_default();
                                            if let Some(n) = menu_next_index(Some(i), row_count, k) {
                                                focus_element_by_id(&format!("ih-agent-opt-{n}"));
                                            }
                                        } else if e.key() == Key::Enter {
                                            e.prevent_default();
                                            pick_agent(
                                                name_for_key.clone(),
                                                tabs,
                                                notice,
                                                refresh,
                                                active_screen,
                                                open,
                                            );
                                        } else if e.key() == Key::Escape {
                                            open.set(false);
                                            focus_element_by_id("ih-agent-trigger");
                                        }
                                    },
                                    span { class: dot_class, "aria-hidden": "true" }
                                    span { class: "ih-agent-opt-name", "{row.name}" }
                                    if row.is_live {
                                        span { class: "ih-agent-marker", "LIVE" }
                                    }
                                    if row.is_open {
                                        span { class: "ih-agent-marker", "OPEN" }
                                    }
                                }
                            }
                        }
                    }
                    if loading {
                        div { class: "ih-agent-note", "Loading agents\u{2026}" }
                    }
                    if let Some(reason) = load_error {
                        div { class: "ih-agent-note", "Couldn't load agents: {reason}" }
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
        AgentNoticeCtx, AgentTabsCtx, AgentTabsState, LiveAgentCtx,
    };
    use crate::components::hermes_app::profile_topbar::{
        SharedProfilesCtx, TopbarProfileRefreshCtx,
    };
    use crate::protocol::{ProfileHealth, ProfileRow};
    use crate::state::Screen;

    fn row(name: &str) -> ProfileRow {
        ProfileRow {
            name: name.to_string(),
            health: ProfileHealth::Configured,
            gaps: Vec::new(),
            provider: Some("openrouter".into()),
            model_default: Some("m".into()),
            key_count: 1,
        }
    }

    #[test]
    fn agent_dropdown_rows_order_default_first() {
        let profiles = vec![row("zeta"), row("alpha")];
        let open = vec!["default".to_string(), "zeta".to_string()];
        let rows = dropdown_rows(&profiles, Some("alpha"), &open);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["default", "alpha", "zeta"]);
        assert!(rows[1].is_live && !rows[0].is_live && !rows[2].is_live);
        assert!(rows[0].is_open && rows[2].is_open && !rows[1].is_open);
        assert!(rows[0].health_ok);
        assert_eq!(rows[0].meta, "Root home \u{2014} no profile override");
    }

    #[test]
    fn agent_dropdown_rows_empty_profile_list_is_only_default() {
        let rows = dropdown_rows(&[], None, &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "default");
    }

    #[test]
    fn agent_dropdown_rows_never_duplicate_the_default_row() {
        let rows = dropdown_rows(&[row("default"), row("dev")], Some("default"), &[]);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["default", "dev"]);
        assert!(rows[0].is_live);
    }

    #[test]
    fn smoke_agent_dropdown_mounts_without_panic() {
        fn test_app() -> Element {
            let screen = use_signal(|| Screen::Kanban);
            use_context_provider(|| screen);
            let tabs = use_signal(AgentTabsState::default_layout);
            use_context_provider(|| AgentTabsCtx(tabs));
            let live = use_resource(|| async { Ok::<String, ServerFnError>("default".to_string()) });
            use_context_provider(|| LiveAgentCtx(live));
            let notice = use_signal(|| None::<String>);
            use_context_provider(|| AgentNoticeCtx(notice));
            let tick = use_signal(|| 0_u32);
            use_context_provider(|| TopbarProfileRefreshCtx(tick));
            let profiles = use_resource(|| async { Ok::<Vec<ProfileRow>, ServerFnError>(Vec::new()) });
            use_context_provider(|| SharedProfilesCtx(profiles));
            rsx! { AgentDropdown {} }
        }

        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_to_vec();
    }
}

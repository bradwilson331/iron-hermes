//! Phase 54.4 Plan 06 (D-10, D-11, D-21, D-26) — the agent tab strip in the
//! Chat band row, beside the docked wheel.
//!
//! The strip is a quick switcher over the SINGLE root chat (D-10): clicking a
//! tab calls [`focus_agent`] (the header strip, env chip and activation follow
//! the focused agent), and nothing here reads or writes the transcript, the
//! chat session or the WebSocket, so focus, close and park can never cancel,
//! hide or reset a running turn (Interaction Rule 3).
//!
//! A tab is `[● live dot] [name] [controls: ⚙ − ✕]`: the name is the `role="tab"`
//! button and the controls are sibling buttons in a `role="group"`, so no
//! interactive element is nested inside the tab role. The tab list uses a
//! roving tabindex (Left/Right wrap, Home/End jump, Enter/Space select).
//!
//! This component consumes the root contexts (`AgentTabsCtx`, `LiveAgentCtx`,
//! `AgentNoticeCtx`, `TopbarProfileRefreshCtx`) and provides none; a provider
//! declared in a child panics its consumers at runtime.

use crate::components::hermes_app::agent_tabs::{
    focus_agent, live_agent_name, set_tabs_recording_focus, AgentNoticeCtx, AgentTabsCtx,
    GuardOutcome, LiveAgentCtx,
};
use crate::components::hermes_app::header::focus_element_by_id;
use crate::components::hermes_app::profile_topbar::TopbarProfileRefreshCtx;
use dioxus::prelude::*;

/// A key that moves the roving tab focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RovingKey {
    Left,
    Right,
    Home,
    End,
}

/// The tab index a roving key lands on: Left/Right wrap, Home/End jump. An
/// empty list always yields 0.
pub(crate) fn roving_next(current: usize, len: usize, key: RovingKey) -> usize {
    if len == 0 {
        return 0;
    }
    let current = current.min(len - 1);
    match key {
        RovingKey::Left => (current + len - 1) % len,
        RovingKey::Right => (current + 1) % len,
        RovingKey::Home => 0,
        RovingKey::End => len - 1,
    }
}

/// Scroll the tab whose name button has DOM id `button_id` into the strip's
/// visible span (wasm: adjusts the strip's own `scrollLeft`, never the page).
#[cfg(target_arch = "wasm32")]
fn scroll_tab_into_view(button_id: &str) {
    use wasm_bindgen::JsCast;
    // Keep the tab clear of the 16px edge fade.
    const FADE: i32 = 16;
    let Some(button) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(button_id))
    else {
        return;
    };
    let Ok(Some(tab)) = button.closest(".ih-tab") else {
        return;
    };
    let Some(strip) = tab.parent_element() else {
        return;
    };
    let Ok(tab) = tab.dyn_into::<web_sys::HtmlElement>() else {
        return;
    };
    let left = tab.offset_left();
    let right = left + tab.offset_width();
    let view_left = strip.scroll_left();
    let view_right = view_left + strip.client_width();
    if left < view_left + FADE {
        strip.set_scroll_left((left - FADE).max(0));
    } else if right > view_right - FADE {
        strip.set_scroll_left(right - strip.client_width() + FADE);
    }
}

/// Native twin of [`scroll_tab_into_view`]: there is no DOM to scroll.
#[cfg(not(target_arch = "wasm32"))]
fn scroll_tab_into_view(button_id: &str) {
    let _ = button_id;
}

/// The Chat band-row tab strip. Renders `role="tablist"` always; with zero
/// tabs it shows the hint instead of tabs and the live chat stays usable.
#[component]
pub fn AgentTabStrip() -> Element {
    let mut tabs = use_context::<AgentTabsCtx>().0;
    let notice = use_context::<AgentNoticeCtx>().0;
    let live_ctx = use_context::<LiveAgentCtx>();
    let refresh = use_context::<TopbarProfileRefreshCtx>().0;
    // The tab holding tabindex=0. `None` follows the focused tab; an arrow key
    // moves it without selecting (manual activation).
    let mut roving = use_signal(|| None::<usize>);

    // Scroll the focused tab into view. Reads `tabs`, writes no signal.
    use_effect(move || {
        let focused = tabs.read().focused.clone();
        if let Some(id) = focused {
            scroll_tab_into_view(&format!("ih-tab-{id}"));
        }
    });

    // Owned locals BEFORE rsx! so no signal borrow is held across the tree.
    let (visible, focused): (Vec<String>, Option<String>) = {
        let state = tabs.read();
        (state.visible_tabs(), state.focused.clone())
    };
    let live = live_agent_name(live_ctx.0());
    let len = visible.len();
    let tab_stop = roving
        .read()
        .filter(|i| *i < len)
        .or_else(|| {
            focused
                .as_ref()
                .and_then(|f| visible.iter().position(|a| a == f))
        })
        .unwrap_or(0);

    rsx! {
        div {
            class: "ih-tabstrip",
            role: "tablist",
            "aria-label": "Agent tabs",
            if visible.is_empty() {
                div { class: "ih-tabstrip-empty",
                    "No agent tabs open. Pick one from the Agent menu."
                }
            }
            for (i , agent) in visible.iter().cloned().enumerate() {
                {
                    let is_focused = focused.as_deref() == Some(agent.as_str());
                    let is_live = live.as_deref() == Some(agent.as_str());
                    let order = visible.clone();
                    let agent_click = agent.clone();
                    let agent_key = agent.clone();
                    let agent_close = agent.clone();
                    let agent_park = agent.clone();
                    let agent_gear = agent.clone();
                    rsx! {
                        div {
                            key: "{agent}",
                            class: "ih-tab",
                            class: if is_focused { "is-focused" },
                            if is_live {
                                span {
                                    class: "ih-tab-live",
                                    title: "Live \u{2014} chat runs here",
                                    "aria-hidden": "true",
                                    "\u{25CF}"
                                }
                            }
                            button {
                                id: "ih-tab-{agent}",
                                r#type: "button",
                                role: "tab",
                                class: "ih-tab-name",
                                "aria-selected": "{is_focused}",
                                tabindex: if i == tab_stop { "0" } else { "-1" },
                                title: "{agent}",
                                onclick: move |_| {
                                    roving.set(None);
                                    focus_agent(tabs, notice, refresh, agent_click.clone());
                                },
                                onkeydown: move |e: KeyboardEvent| {
                                    let nav = match e.key() {
                                        Key::ArrowLeft => Some(RovingKey::Left),
                                        Key::ArrowRight => Some(RovingKey::Right),
                                        Key::Home => Some(RovingKey::Home),
                                        Key::End => Some(RovingKey::End),
                                        _ => None,
                                    };
                                    if let Some(k) = nav {
                                        e.prevent_default();
                                        let next = roving_next(i, order.len(), k);
                                        roving.set(Some(next));
                                        if let Some(target) = order.get(next) {
                                            focus_element_by_id(&format!("ih-tab-{target}"));
                                        }
                                    } else if e.key() == Key::Enter
                                        || e.key() == Key::Character(" ".to_string())
                                    {
                                        e.prevent_default();
                                        roving.set(None);
                                        focus_agent(tabs, notice, refresh, agent_key.clone());
                                    }
                                },
                                "{agent}"
                            }
                            div {
                                class: "ih-tab-controls",
                                role: "group",
                                "aria-label": "{agent} actions",
                                button {
                                    r#type: "button",
                                    id: "ih-tab-gear-{agent}",
                                    class: "ih-tab-ctl",
                                    "aria-label": "Settings for {agent}",
                                    title: "Settings for {agent}",
                                    onclick: move |_| {
                                        // D-16: open the slide-over for this agent (only
                                        // writes when it was closed), then focus the tab
                                        // so the header strip follows (Interaction Rule 2).
                                        let mut next = tabs.peek().clone();
                                        if next.open_settings(&agent_gear) {
                                            tabs.set(next);
                                        }
                                        focus_agent(tabs, notice, refresh, agent_gear.clone());
                                    },
                                    "\u{2699}"
                                }
                                button {
                                    r#type: "button",
                                    class: "ih-tab-ctl",
                                    "aria-label": "Park {agent}",
                                    title: "Park {agent}",
                                    onclick: move |_| {
                                        let mut next = tabs.peek().clone();
                                        if next.park(&agent_park) {
                                            // Parking the focused tab moves focus: record it (WR-02).
                                            set_tabs_recording_focus(tabs, notice, refresh, next);
                                        }
                                    },
                                    "\u{2212}"
                                }
                                button {
                                    r#type: "button",
                                    class: "ih-tab-ctl",
                                    "aria-label": "Close {agent}",
                                    title: "Close {agent}",
                                    onclick: move |_| {
                                        // Dirty guard: closing the agent whose settings
                                        // panel holds unsaved edits asks first.
                                        let mut next = tabs.peek().clone();
                                        if next.request_close(&agent_close)
                                            != GuardOutcome::Unchanged
                                        {
                                            // Closing the focused tab moves focus: record it (WR-02).
                                            set_tabs_recording_focus(tabs, notice, refresh, next);
                                        }
                                    },
                                    "\u{2715}"
                                }
                            }
                        }
                    }
                }
            }
        }
        LogsPanelToggle {}
    }
}

/// The LOGS PANEL toggle at the strip's right end (D-14): a sibling of the
/// scrolling tab list inside `.ih-tabstrip-row`, so it never scrolls away.
/// `aria-pressed` mirrors `AgentTabsState::logs_widget`; the Logs panel
/// itself is mounted by the widget column.
#[component]
fn LogsPanelToggle() -> Element {
    let mut tabs = use_context::<AgentTabsCtx>().0;
    let on = tabs.read().logs_widget;
    rsx! {
        button {
            r#type: "button",
            class: "ih-logs-toggle",
            "aria-pressed": "{on}",
            onclick: move |_| {
                // Write only when changed.
                let mut next = tabs.peek().clone();
                let want = !next.logs_widget;
                if next.set_logs_widget(want) {
                    tabs.set(next);
                }
            },
            "LOGS PANEL"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::hermes_app::agent_tabs::{
        AgentNoticeCtx, AgentTabsCtx, AgentTabsState, LiveAgentCtx,
    };
    use crate::components::hermes_app::profile_topbar::TopbarProfileRefreshCtx;
    use crate::state::Screen;

    #[test]
    fn tab_strip_roving_next_wraps() {
        assert_eq!(roving_next(0, 3, RovingKey::Left), 2);
        assert_eq!(roving_next(2, 3, RovingKey::Right), 0);
        assert_eq!(roving_next(1, 3, RovingKey::Home), 0);
        assert_eq!(roving_next(1, 3, RovingKey::End), 2);
        assert_eq!(roving_next(0, 1, RovingKey::Right), 0);
        assert_eq!(roving_next(0, 0, RovingKey::Right), 0);
    }

    #[test]
    fn smoke_tab_strip_mounts_without_panic() {
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
            rsx! { AgentTabStrip {} }
        }

        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_to_vec();
    }
}

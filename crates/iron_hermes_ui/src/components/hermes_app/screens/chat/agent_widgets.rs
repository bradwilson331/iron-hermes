//! Phase 54.4 Plan 06 (D-13, D-14) — the Chat page's left widget column of
//! parked agent cards.
//!
//! The column renders ONLY while at least one tab is parked or the Logs
//! mini-panel is on (D-13); otherwise the chat spans the full width. Each card shows the agent name, a `PARKED`
//! label and the live dot when that agent is live. Clicking the card body
//! restores the tab and focuses it (via `focus_agent`); the corner `✕` closes
//! the agent (same as the tab's close, D-11).
//!
//! Cards show NO live per-agent status (idle/running/needs approval): that is
//! deferred to Phase 54.6. This module consumes only `AgentTabsCtx`,
//! `LiveAgentCtx` and the notice/refresh contexts `focus_agent` needs, and
//! provides none; it never touches the chat singletons (D-10).

use crate::components::hermes_app::agent_tabs::{
    focus_agent, live_agent_name, set_tabs_recording_focus, AgentNoticeCtx, AgentTabsCtx,
    GuardOutcome, LiveAgentCtx,
};
use crate::components::hermes_app::profile_topbar::TopbarProfileRefreshCtx;
use dioxus::prelude::*;

/// One parked agent card.
#[component]
fn ParkedCard(agent: String, is_live: bool) -> Element {
    let tabs = use_context::<AgentTabsCtx>().0;
    let notice = use_context::<AgentNoticeCtx>().0;
    let refresh = use_context::<TopbarProfileRefreshCtx>().0;
    let agent_restore = agent.clone();
    let agent_close = agent.clone();

    rsx! {
        div { class: "ih-parked-card",
            button {
                r#type: "button",
                class: "ih-parked-body",
                title: "{agent}",
                onclick: move |_| {
                    focus_agent(tabs, notice, refresh, agent_restore.clone());
                },
                span { class: "ih-parked-name", "{agent}" }
                span { class: "ih-parked-label",
                    if is_live {
                        span {
                            class: "ih-tab-live",
                            title: "Live \u{2014} chat runs here",
                            "aria-hidden": "true",
                            "\u{25CF} "
                        }
                    }
                    "PARKED"
                }
            }
            button {
                r#type: "button",
                class: "ih-parked-close",
                "aria-label": "Close {agent}",
                title: "Close {agent}",
                onclick: move |_| {
                    // Dirty guard: closing the agent whose settings panel
                    // holds unsaved edits asks first.
                    let mut next = tabs.peek().clone();
                    if next.request_close(&agent_close) != GuardOutcome::Unchanged {
                        // Closing the focused card moves focus: record it (WR-02).
                        set_tabs_recording_focus(tabs, notice, refresh, next);
                    }
                },
                "\u{2715}"
            }
        }
    }
}

/// The widget column: parked-agent cards. Renders nothing while nothing is
/// parked (D-13).
#[component]
pub fn AgentWidgetColumn() -> Element {
    let tabs = use_context::<AgentTabsCtx>().0;
    let live_ctx = use_context::<LiveAgentCtx>();

    // Owned locals BEFORE rsx! so no signal borrow is held across the tree.
    let (shown, parked, logs_on) = {
        let state = tabs.read();
        (
            state.widget_column_visible(),
            state.parked.clone(),
            state.logs_widget,
        )
    };
    let live = live_agent_name(live_ctx.0());

    if !shown {
        return rsx! {};
    }

    rsx! {
        aside { class: "ih-widget-col", "aria-label": "Widgets",
            div { class: "ih-parked-cards",
                for agent in parked {
                    ParkedCard {
                        key: "{agent}",
                        is_live: live.as_deref() == Some(agent.as_str()),
                        agent,
                    }
                }
            }
            // Phase 54.4 Plan 11 (D-14/D-25): the second widget kind, below
            // the parked cards. Owns its own contexts and rows.
            if logs_on {
                super::logs_mini_panel::LogsMiniPanel {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::hermes_app::agent_tabs::AgentTabsState;

    const AGENT_WIDGETS_RS: &str = include_str!("agent_widgets.rs");

    #[test]
    fn smoke_agent_widget_column_mounts_without_panic() {
        fn test_app() -> Element {
            let tabs = use_signal(|| AgentTabsState {
                open: vec!["default".to_string(), "dev".to_string()],
                focused: Some("default".to_string()),
                parked: vec!["dev".to_string()],
                ..Default::default()
            });
            use_context_provider(|| AgentTabsCtx(tabs));
            let live =
                use_resource(|| async { Ok::<String, ServerFnError>("dev".to_string()) });
            use_context_provider(|| LiveAgentCtx(live));
            let notice = use_signal(|| None::<String>);
            use_context_provider(|| AgentNoticeCtx(notice));
            let tick = use_signal(|| 0_u32);
            use_context_provider(|| TopbarProfileRefreshCtx(tick));
            rsx! { AgentWidgetColumn {} }
        }

        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_to_vec();
    }

    /// 54.6 deferral (D-14): cards carry no per-agent run status, so the
    /// module consumes only the tab, live-agent, notice and refresh contexts.
    #[test]
    fn agent_widgets_consume_only_the_tab_live_and_notice_contexts() {
        let code = &AGENT_WIDGETS_RS[..AGENT_WIDGETS_RS
            .find("#[cfg(test)]")
            .unwrap_or(AGENT_WIDGETS_RS.len())];
        let allowed = ["AgentTabsCtx", "LiveAgentCtx", "AgentNoticeCtx", "TopbarProfileRefreshCtx"];
        for line in code.lines().filter(|l| l.contains("use_context::<")) {
            assert!(
                allowed.iter().any(|a| line.contains(a)),
                "unexpected context consumed by the widget column: {line}"
            );
        }
    }
}

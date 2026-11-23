//! Phase 54.4 Plan 06 (D-21) — the "chat runs on the live agent" banner.
//!
//! The chat is one root-level chat that always runs on the live agent (D-10).
//! When the focused tab is a different agent, the strip, settings and env
//! display follow the tab but the transcript does not, so this banner says
//! plainly where the chat runs and offers a button that focuses the live
//! agent's tab. It only DISPLAYS: the chat below keeps running exactly as it
//! was, and nothing here reads or writes it. It consumes the root contexts and
//! provides none.

use crate::components::hermes_app::agent_tabs::{
    focus_agent, live_agent_name, AgentNoticeCtx, AgentTabsCtx, LiveAgentCtx, DEFAULT_AGENT,
};
use crate::components::hermes_app::profile_topbar::TopbarProfileRefreshCtx;
use dioxus::prelude::*;

/// What is known about the live agent's name.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum LiveName {
    Known(String),
    /// The live resource is unresolved or failed.
    Unknown,
}

/// The banner's text and, when the live agent's name is known, the agent its
/// button focuses.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct BannerCopy {
    pub text: String,
    pub go_to: Option<String>,
}

/// The banner to show for `focused` given the live agent, or `None` when the
/// focused tab is the live agent (or nothing is focused). With an unknown live
/// name the banner reads generically and has no button; the root agent is
/// never called out in that state.
pub(crate) fn banner_state(focused: Option<&str>, live: LiveName) -> Option<BannerCopy> {
    let focused = focused?;
    match live {
        LiveName::Known(live) if live == focused => None,
        LiveName::Known(live) => Some(BannerCopy {
            text: format!(
                "Chat runs on {live}. This tab shows {focused}'s settings and status only."
            ),
            go_to: Some(live),
        }),
        LiveName::Unknown if focused == DEFAULT_AGENT => None,
        LiveName::Unknown => Some(BannerCopy {
            text: "Chat runs on the live agent.".to_string(),
            go_to: None,
        }),
    }
}

/// The non-live tab banner: a fixed 40px single line whose text ellipsizes
/// before the (never clipped) button.
#[component]
pub fn NonLiveBanner() -> Element {
    let tabs = use_context::<AgentTabsCtx>().0;
    let notice = use_context::<AgentNoticeCtx>().0;
    let live_ctx = use_context::<LiveAgentCtx>();
    let refresh = use_context::<TopbarProfileRefreshCtx>().0;

    // Owned locals BEFORE rsx! so no signal borrow is held across the tree.
    let focused = tabs.read().focused.clone();
    let live = match live_agent_name(live_ctx.0()) {
        Some(name) => LiveName::Known(name),
        None => LiveName::Unknown,
    };
    let Some(copy) = banner_state(focused.as_deref(), live) else {
        return rsx! {};
    };
    let BannerCopy { text, go_to } = copy;

    rsx! {
        div { class: "ih-nonlive-banner", role: "status",
            span { class: "ih-nonlive-text", title: "{text}", "{text}" }
            if let Some(live_name) = go_to {
                button {
                    r#type: "button",
                    class: "ih-btn-secondary",
                    title: "Go to {live_name}",
                    onclick: {
                        let target = live_name.clone();
                        move |_| focus_agent(tabs, notice, refresh, target.clone())
                    },
                    "Go to {live_name}"
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
    use crate::components::hermes_app::profile_topbar::TopbarProfileRefreshCtx;

    #[test]
    fn non_live_banner_hidden_for_the_live_agent() {
        assert!(banner_state(Some("default"), LiveName::Known("default".into())).is_none());
    }

    #[test]
    fn non_live_banner_shown_for_a_non_live_agent() {
        let b = banner_state(Some("dev"), LiveName::Known("default".into()))
            .expect("a non-live tab shows the banner");
        assert_eq!(
            b.text,
            "Chat runs on default. This tab shows dev's settings and status only."
        );
        assert_eq!(b.go_to.as_deref(), Some("default"));
    }

    #[test]
    fn non_live_banner_unknown_live_name() {
        let b = banner_state(Some("dev"), LiveName::Unknown).expect("banner still shows");
        assert_eq!(b.text, "Chat runs on the live agent.");
        assert_eq!(b.go_to, None);
        // The root agent is never called out when the live name is unknown.
        assert!(banner_state(Some("default"), LiveName::Unknown).is_none());
    }

    #[test]
    fn non_live_banner_hidden_when_nothing_focused() {
        assert!(banner_state(None, LiveName::Known("default".into())).is_none());
    }

    #[test]
    fn smoke_non_live_banner_mounts_without_panic() {
        fn test_app() -> Element {
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
            rsx! { NonLiveBanner {} }
        }

        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_to_vec();
    }
}

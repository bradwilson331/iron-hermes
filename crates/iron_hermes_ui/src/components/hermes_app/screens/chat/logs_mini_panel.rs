//! Phase 54.4 Plan 11 (D-14, D-25) — the Logs mini-panel in the Chat widget
//! column.
//!
//! Shows the last [`MINI_PANEL_ROWS`] CRITICAL agent-activity rows (blocks,
//! approvals, failures) of the FOCUSED agent only, read reactively from the
//! client-side `ActivityLogCtx` ring that plan 07 feeds from the chat
//! WebSocket loop. Phase 57 D-18 made the rail critical-only
//! (`ActivityEventKind::is_critical`): routine turn, tool-call and subagent rows
//! are not listed here, and the Logs screen's AGENT ACTIVITY tab stays the
//! complete feed. It shows no audit rows and no server-log lines (those carry
//! no agent attribution, D-24/D-25), makes no network request and has no
//! timer, so there is nothing to poll, starve or leak when it unmounts.
//!
//! This component consumes the root contexts (`AgentTabsCtx`, `ActivityLogCtx`,
//! `WsConnectedContext`, `Signal<Screen>`) and provides none; a provider
//! declared in a child panics its consumers at runtime. All text reaches the
//! DOM as text nodes and attributes only (T-54.4-38).

use crate::activity_log::{ActivityLogCtx, ActivityRow};
use crate::components::hermes_app::agent_tabs::{AgentTabsCtx, DEFAULT_AGENT};
use crate::components::hermes_app::screens::logs::{
    filter_activity, is_near_bottom, scroll_to_bottom,
};
use crate::state::{Screen, WsConnectedContext};
use dioxus::prelude::*;

/// How many rows the panel keeps (UI-SPEC E9 zero-one-many).
const MINI_PANEL_ROWS: usize = 20;

/// DOM id of the scrolling body (the auto-follow helpers address it by id).
const BODY_ID: &str = "ih-logs-mini-body";

/// The focused agent's newest [`MINI_PANEL_ROWS`] CRITICAL activity rows
/// ([`ActivityEventKind::is_critical`](crate::activity_log::ActivityEventKind::is_critical):
/// critical events, approvals and errors), oldest first (newest at the bottom). Routine
/// turn and tool rows are left out here; the Logs screen still lists them (D-18).
pub(crate) fn mini_panel_rows(rows: &[ActivityRow], agent: &str) -> Vec<ActivityRow> {
    let mut mine = filter_activity(rows, Some(agent));
    mine.retain(|r| r.kind.is_critical());
    let excess = mine.len().saturating_sub(MINI_PANEL_ROWS);
    mine.drain(..excess);
    mine
}

/// The Logs mini-panel. Mounted by the widget column while
/// `AgentTabsState::logs_widget` is on.
#[component]
pub fn LogsMiniPanel() -> Element {
    let mut tabs = use_context::<AgentTabsCtx>().0;
    let ring = use_context::<ActivityLogCtx>().0;
    let ws = use_context::<WsConnectedContext>().0;
    let mut screen = use_context::<Signal<Screen>>();
    // Whether the body is pinned to the newest row. Maintained by `onscroll`;
    // the effect below only reads it.
    let mut following = use_signal(|| true);

    // Auto-follow: keep the body at the bottom while `following`. Reads the
    // newest row's sequence number (the ring's length stops changing once it
    // is capped), the focused agent and `following`; writes no signal, so it
    // cannot retrigger itself.
    use_effect(move || {
        let _newest = ring.read().last().map(|r| r.seq);
        let _agent = tabs.read().focused.clone();
        if *following.read() {
            scroll_to_bottom(BODY_ID);
        }
    });

    // Owned locals BEFORE rsx! so no signal borrow is held across the tree.
    let agent = tabs
        .read()
        .focused
        .clone()
        .unwrap_or_else(|| DEFAULT_AGENT.to_string());
    let rows = mini_panel_rows(&ring.read(), &agent);
    let connected = *ws.read();

    rsx! {
        section { class: "ih-logs-mini", "aria-label": "Logs panel",
            div { class: "ih-logs-mini-head",
                span { class: "ih-logs-mini-title", title: "{agent}", "LOGS \u{00B7} {agent}" }
                button {
                    r#type: "button",
                    class: "ih-logs-mini-close",
                    "aria-label": "Hide Logs panel",
                    title: "Hide Logs panel",
                    onclick: move |_| {
                        let mut next = tabs.peek().clone();
                        if next.set_logs_widget(false) {
                            tabs.set(next);
                        }
                    },
                    "\u{2715}"
                }
            }
            if !connected {
                div { class: "ih-logs-mini-note", role: "status",
                    "Disconnected. Activity resumes when the chat reconnects."
                }
            }
            div {
                id: "{BODY_ID}",
                class: "ih-logs-mini-body",
                role: "log",
                "aria-live": "off",
                onscroll: move |_| {
                    let near = is_near_bottom(BODY_ID, 24);
                    if *following.peek() != near {
                        following.set(near);
                    }
                },
                if rows.is_empty() {
                    div { class: "ih-log-empty",
                        div { class: "ih-log-empty-heading", "No critical events" }
                        div { class: "ih-log-empty-body",
                            "Blocks, approvals and failures for {agent} appear here. Routine tool calls are on the Logs screen."
                        }
                    }
                }
                for r in rows {
                    {
                        let full = format!("{}  {}  {}", r.hms, r.kind.label(), r.summary);
                        rsx! {
                            div { key: "{r.seq}", class: "ih-log-row", title: "{full}", "{full}" }
                        }
                    }
                }
            }
            div { class: "ih-logs-mini-foot",
                button {
                    r#type: "button",
                    class: "ih-link",
                    onclick: move |_| screen.set(Screen::Logs),
                    "Open Logs"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity_log::{ActivityEventKind, ActivityLogCtx, ActivityRow};
    use crate::components::hermes_app::agent_tabs::{AgentTabsCtx, AgentTabsState};
    use crate::state::{Screen, WsConnectedContext};

    const MINI_PANEL_RS: &str = include_str!("logs_mini_panel.rs");

    fn row(seq: u64, agent: &str) -> ActivityRow {
        ActivityRow {
            seq,
            hms: "12:00:00".to_string(),
            agent: agent.to_string(),
            kind: ActivityEventKind::Critical,
            summary: format!("event {seq}"),
        }
    }

    #[test]
    fn mini_panel_rows_filters_focused_agent_and_caps_20() {
        let mut rows = Vec::new();
        for i in 0..30 {
            rows.push(row(i, "dev"));
        }
        for i in 30..35 {
            rows.push(row(i, "ops"));
        }
        let shown = mini_panel_rows(&rows, "dev");
        assert_eq!(shown.len(), 20, "capped at 20");
        assert!(shown.iter().all(|r| r.agent == "dev"), "focused agent only");
        let seqs: Vec<u64> = shown.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, (10..30).collect::<Vec<u64>>(), "newest 20, oldest first");
        let few = mini_panel_rows(&rows[30..], "ops");
        assert_eq!(few.len(), 5, "fewer than 20 are all kept");
        assert!(mini_panel_rows(&rows, "nobody").is_empty());
    }

    /// D-25 / T-54.4-39: the panel reads the client ring only. Needles are
    /// assembled so this test never matches itself.
    #[test]
    fn mini_panel_makes_no_server_request() {
        let code = &MINI_PANEL_RS[..MINI_PANEL_RS.find("#[cfg(test)]").unwrap_or(MINI_PANEL_RS.len())];
        let needles = [
            ["crate::server", "::"].concat(),
            ["read_log", "_tail"].concat(),
            ["use_server", "_future"].concat(),
            ["use_", "resource"].concat(),
            ["spawn", "("].concat(),
        ];
        for n in &needles {
            assert!(!code.contains(n.as_str()), "mini panel must not contain `{n}`");
        }
    }

    /// The panel, toggle and drop-zone styles live in logs.css (loaded
    /// unconditionally since plan 07) and hide below 900px.
    #[test]
    fn logs_css_styles_the_mini_panel_toggle_and_drop_zone() {
        const LOGS_CSS: &str = include_str!("../../../../../assets/logs.css");
        for needle in [
            ".ih-logs-mini {",
            ".ih-logs-mini-head",
            ".ih-logs-mini-body",
            ".ih-logs-toggle",
            ".ih-chat-body.is-logs-drop-target",
            ".ih-logs-drop-hint",
            "1px dashed var(--teal)",
            "min-height: 192px",
            "@media (max-width: 899px)",
        ] {
            assert!(LOGS_CSS.contains(needle), "logs.css must contain `{needle}`");
        }
        let narrow = &LOGS_CSS[LOGS_CSS.rfind("@media (max-width: 899px)").unwrap()..];
        assert!(narrow.contains(".ih-logs-mini") && narrow.contains(".ih-logs-toggle"));
    }

    fn row_of(seq: u64, agent: &str, kind: ActivityEventKind) -> ActivityRow {
        ActivityRow {
            seq,
            hms: "12:00:00".to_string(),
            agent: agent.to_string(),
            kind,
            summary: format!("row {seq}"),
        }
    }

    /// A ring of 10 routine rows and 2 critical ones for agent `dev`.
    fn mixed_ring() -> Vec<ActivityRow> {
        use ActivityEventKind as K;
        let routine = [
            K::TurnStarted,
            K::ToolStart,
            K::ToolEnd,
            K::YoloBypass,
            K::Subagent,
            K::TurnEnded,
            K::ToolStart,
            K::ToolEnd,
            K::TurnCancelled,
            K::ToolStart,
        ];
        let mut ring: Vec<ActivityRow> = routine
            .iter()
            .enumerate()
            .map(|(i, k)| row_of(i as u64, "dev", *k))
            .collect();
        ring.push(row_of(10, "dev", K::Critical));
        ring.push(row_of(11, "dev", K::Approval));
        ring
    }

    /// D-18: the rail keeps only critical rows (Critical, Approval, Error).
    #[test]
    fn mini_panel_shows_only_critical() {
        let ring = mixed_ring();
        let shown = mini_panel_rows(&ring, "dev");
        let seqs: Vec<u64> = shown.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![10, 11], "exactly the two critical rows");

        let many: Vec<ActivityRow> = (0..25)
            .map(|i| row_of(i, "dev", ActivityEventKind::Critical))
            .collect();
        let shown = mini_panel_rows(&many, "dev");
        let seqs: Vec<u64> = shown.iter().map(|r| r.seq).collect();
        assert_eq!(
            seqs,
            (5..25).collect::<Vec<u64>>(),
            "newest 20, oldest first"
        );
    }

    /// D-18: the Logs screen is NOT filtered; every routine tool row stays listed.
    #[test]
    fn logs_screen_still_lists_routine_tool_rows() {
        let ring = mixed_ring();
        assert_eq!(ring.len(), 12);
        assert_eq!(filter_activity(&ring, None).len(), 12);
        assert_eq!(filter_activity(&ring, Some("dev")).len(), 12);
    }

    /// R7: with only routine rows the rail shows the critical-only empty state.
    #[test]
    fn mini_panel_empty_state_copy() {
        fn test_app() -> Element {
            let screen = use_signal(|| Screen::Chat);
            use_context_provider(|| screen);
            let tabs = use_signal(|| AgentTabsState {
                open: vec!["default".to_string(), "dev".to_string()],
                focused: Some("dev".to_string()),
                logs_widget: true,
                ..Default::default()
            });
            use_context_provider(|| AgentTabsCtx(tabs));
            let ring = use_signal(|| {
                (0..5)
                    .map(|i| row_of(i, "dev", ActivityEventKind::ToolStart))
                    .collect::<Vec<_>>()
            });
            use_context_provider(|| ActivityLogCtx(ring));
            let ws = use_signal(|| true);
            use_context_provider(|| WsConnectedContext(ws));
            rsx! { LogsMiniPanel {} }
        }
        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_in_place();
        let html = dioxus::ssr::render(&dom);
        assert!(html.contains("No critical events"), "{html}");
        assert!(
            html.contains(
                "Blocks, approvals and failures for dev appear here. Routine tool calls are on the Logs screen."
            ),
            "{html}"
        );
        assert!(html.contains("Open Logs"));
        assert!(
            !html.contains(concat!("No recent", " activity")),
            "the old heading must be gone: {html}"
        );
        assert!(!html.contains("row 0"), "no routine row is listed");
    }

    #[test]
    fn smoke_logs_mini_panel_mounts_without_panic() {
        fn test_app() -> Element {
            let screen = use_signal(|| Screen::Chat);
            use_context_provider(|| screen);
            let tabs = use_signal(|| AgentTabsState {
                open: vec!["default".to_string(), "dev".to_string()],
                focused: Some("dev".to_string()),
                logs_widget: true,
                ..Default::default()
            });
            use_context_provider(|| AgentTabsCtx(tabs));
            let ring = use_signal(|| vec![row(1, "dev"), row(2, "default")]);
            use_context_provider(|| ActivityLogCtx(ring));
            let ws = use_signal(|| true);
            use_context_provider(|| WsConnectedContext(ws));
            rsx! { LogsMiniPanel {} }
        }

        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_to_vec();
    }
}

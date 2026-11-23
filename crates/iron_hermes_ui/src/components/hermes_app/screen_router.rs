//! Class-toggle screen router (RESEARCH Pattern 7).
//!
//! Mounts all 16 screen components simultaneously. Only the one matching
//! the context-provided `Signal<Screen>` carries the `is-active` class —
//! every other screen stays mounted but invisible. This matches
//! `app.html`'s native SPA pattern and avoids any WebSocket-teardown
//! problem when the user navigates (RESEARCH Pitfall 5).
//!
//! The fan-out is laid out in the canonical Plan-04-wedge order followed
//! by the three off-wheel screens (Soul, Schedules, Office), Settings +
//! Providers, the Kanban screen added in Phase 36.3.7.11 Plan 04 per D-02
//! (Plan 01 of that phase replaced `ScreenKanban`'s placeholder body with
//! the live KanbanBoard), and finally the Artifacts gallery + viewer pair
//! added in Phase 46.6 Plan 05 (D-07) — reached from Sessions, not a
//! wheel wedge.

use super::agent_tabs::{effective_screen, AgentTabsCtx, GuardOutcome, PanelPending};
use super::screens;
use crate::state::Screen;
use dioxus::prelude::*;

#[component]
pub fn ScreenRouter() -> Element {
    let active = use_context::<Signal<crate::state::Screen>>();
    let mut tabs = use_context::<AgentTabsCtx>().0;

    // Phase 54.4 Plan 09 (T-54.4-31): render the EFFECTIVE screen, not the raw
    // requested one. A dirty settings panel keeps Chat (and its unsaved
    // working copy) mounted while a navigation away is pending; clean or
    // closed panels pass the request straight through. A memo, so tab-model
    // changes that do not alter the rendered screen never re-run this body.
    let cur_screen = use_memo(move || effective_screen(*active.read(), &tabs.read().panel));
    let cur = cur_screen();

    // Record a navigation the dirty guard is holding so the panel can ask
    // "Discard changes?". Subscribes to the requested screen only: the tab
    // model is PEEKED, so this effect cannot retrigger itself. Clean and
    // closed panels are `Applied` and write nothing.
    use_effect(move || {
        let target = *active.read();
        let mut next = tabs.peek().clone();
        match next.request_navigate(target) {
            GuardOutcome::Applied => {}
            GuardOutcome::Pending => {
                if tabs.peek().panel.pending != next.panel.pending {
                    tabs.set(next);
                }
            }
            // Back on Chat: a stale pending navigation is moot.
            GuardOutcome::Unchanged => {
                if matches!(next.panel.pending, Some(PanelPending::Navigate(_))) {
                    next.panel.pending = None;
                    tabs.set(next);
                }
            }
        }
    });

    // Phase 49.4 hotfix — render ONLY the active screen.
    //
    // Originally all 16 screens mounted at once (RESEARCH Pattern 7). The
    // first hotfix made them mount lazily but STAY mounted, which meant a full
    // tour of the wheel re-accumulated every screen's fetches, polls, and
    // continuous Three.js/WebGL render loops (the voice orb in ScreenChat, the
    // bot avatars in ScreenAgents) — the same saturation, just deferred until
    // you had visited everything. On a single-threaded WASM client the only
    // reliable bound is to keep exactly ONE screen mounted: the active one.
    //
    // Pattern 7's reason for keeping screens mounted was WebSocket-teardown
    // avoidance, but `use_websocket` lives at the `HermesApp` root (mod.rs),
    // NOT in a screen, and the chat transcript is a root-level signal — so
    // unmounting an inactive screen never touches the socket or loses the
    // conversation. The cost is that a screen re-fetches when you return to it
    // and loses purely-local transient state (e.g. an unsent composer draft);
    // that is the correct trade for a client that otherwise freezes. Each
    // rendered screen is, by construction, the active one, so `is_active` is
    // always true here.
    match cur {
        Screen::Chat => rsx! { screens::chat::ScreenChat { is_active: true } },
        Screen::Sessions => rsx! { screens::sessions::ScreenSessions { is_active: true } },
        Screen::Agents => rsx! { screens::agents::ScreenAgents { is_active: true } },
        Screen::Skills => rsx! { screens::skills::ScreenSkills { is_active: true } },
        Screen::Models => rsx! { screens::models::ScreenModels { is_active: true } },
        Screen::Memory => rsx! { screens::memory::ScreenMemory { is_active: true } },
        Screen::Soul => rsx! { screens::soul::ScreenSoul { is_active: true } },
        Screen::Tools => rsx! { screens::tools::ScreenTools { is_active: true } },
        Screen::Schedules => rsx! { screens::schedules::ScreenSchedules { is_active: true } },
        Screen::Gateway => rsx! { screens::gateway::ScreenGateway { is_active: true } },
        Screen::Office => rsx! { screens::office::ScreenOffice { is_active: true } },
        Screen::Settings => rsx! { screens::settings::ScreenSettings { is_active: true } },
        Screen::Providers => rsx! { screens::providers::ScreenProviders { is_active: true } },
        // Phase 36.3.7.11 D-02: Kanban screen (wheel wedge + Agents-page
        // `KANBAN BOARD →` button drive active_screen).
        Screen::Kanban => rsx! { screens::kanban::ScreenKanban { is_active: true } },
        // Phase 46.6 Plan 05 (D-07): artifacts gallery + sandboxed viewer,
        // reached from the Sessions screen's `▤ ARTIFACTS` button.
        Screen::Artifacts => rsx! { screens::artifacts::ScreenArtifacts { is_active: true } },
        Screen::ArtifactViewer => rsx! { screens::artifact_viewer::ArtifactViewer { is_active: true } },
        // Phase 54.4 Plan 07 (D-08): server / audit / agent-activity logs,
        // reached from the header's Logs shortcut (not a wheel wedge).
        Screen::Logs => rsx! { screens::logs::ScreenLogs { is_active: true } },
        // Phase 54.9 Plan 09 (D-13): generated views in a sandboxed frame, reached from a
        // header shortcut (menu catalog entry `openui`).
        Screen::OpenUi => rsx! { screens::openui::ScreenOpenUi { is_active: true } },
        // Phase 54.10 Plan 01 (D-01): live boards, reached from a header shortcut (menu
        // catalog entry `dashboard`).
        Screen::Dashboard => rsx! { screens::dashboard::ScreenDashboard { is_active: true } },
    }
}

#[cfg(test)]
mod tests {
    const ROUTER: &str = include_str!("screen_router.rs");
    const FOOTER: &str = include_str!("app_footer.rs");

    /// Plan 09: ScreenRouter must match on the screen it RENDERS, not the raw
    /// requested one, so a dirty settings panel keeps Chat (and its working
    /// copy) mounted while a navigation is pending.
    #[test]
    fn screen_router_renders_through_effective_screen() {
        let code = &ROUTER[..ROUTER.find("#[cfg(test)]").unwrap_or(ROUTER.len())];
        let call = ["effective_", "screen("].concat();
        let guard = ["request_", "navigate("].concat();
        let raw_match = ["match *active", ".read()"].concat();
        assert!(code.contains(&call), "the router must call effective_screen");
        assert!(code.contains(&guard), "the router must record pending navigation");
        assert!(
            !code.contains(&raw_match),
            "the router must not match on the raw requested screen"
        );
        assert!(
            code.lines().any(|l| l.trim() == "match cur {"),
            "the screen fan-out must match on `cur`, the effective screen"
        );
        let cur_decl = code.split("let cur").nth(1).expect("the router must bind `cur`");
        assert!(
            cur_decl.split(';').next().unwrap_or("").contains(&call),
            "`cur` must come from effective_screen"
        );
    }

    #[test]
    fn screen_logs_router_and_footer_arms() {
        let needle = [
            "Screen::Logs => rsx! { screens::logs::",
            "ScreenLogs { is_active: true } }",
        ]
        .concat();
        assert!(
            ROUTER.contains(&needle),
            "router must mount ScreenLogs for Screen::Logs"
        );
        assert!(
            FOOTER.contains("Screen::Logs =>"),
            "footer must label Screen::Logs"
        );
    }

    /// Phase 54.9 Plan 09 (D-13): the OpenUi arm is named in the router, footer and breadcrumb.
    #[test]
    fn screen_openui_router_footer_breadcrumb_arms() {
        let needle = [
            "Screen::OpenUi => rsx! { screens::openui::",
            "ScreenOpenUi { is_active: true } }",
        ]
        .concat();
        assert!(ROUTER.contains(&needle), "router must mount ScreenOpenUi");
        assert!(
            FOOTER.contains("Screen::OpenUi => \"OPENUI\""),
            "footer must label Screen::OpenUi"
        );
        assert_eq!(
            crate::components::hermes_app::breadcrumb::screen_short_name(
                crate::state::Screen::OpenUi
            ),
            "OPENUI"
        );
    }

    /// Phase 54.10 Plan 01 (D-01): the Dashboard arm is named in the router, footer and breadcrumb.
    #[test]
    fn screen_dashboard_router_footer_breadcrumb_arms() {
        let needle = [
            "Screen::Dashboard => rsx! { screens::dashboard::",
            "ScreenDashboard { is_active: true } }",
        ]
        .concat();
        assert!(
            ROUTER.contains(&needle),
            "router must mount ScreenDashboard"
        );
        let footer_needle = ["Screen::Dashboard => ", "\"DASHBOARD\""].concat();
        assert!(
            FOOTER.contains(&footer_needle),
            "footer must label Screen::Dashboard"
        );
        assert_eq!(
            crate::components::hermes_app::breadcrumb::screen_short_name(
                crate::state::Screen::Dashboard
            ),
            "DASHBOARD"
        );
    }
}

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

use super::screens;
use crate::state::Screen;
use dioxus::prelude::*;

#[component]
pub fn ScreenRouter() -> Element {
    let active = use_context::<Signal<crate::state::Screen>>();
    // Drop the borrow immediately (clippy signal-borrow-safety rule).
    let cur = *active.read();

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
    }
}

//! Class-toggle screen router (RESEARCH Pattern 7).
//!
//! Mounts all 13 screen placeholder components simultaneously. Only the
//! one matching the context-provided `Signal<Screen>` carries the
//! `is-active` class — every other screen stays mounted but invisible.
//! This matches `app.html`'s native SPA pattern and avoids any
//! WebSocket-teardown problem when the user navigates (RESEARCH Pitfall
//! 5).
//!
//! The 13-way fan-out is laid out in the canonical Plan-04-wedge order
//! followed by the three off-wheel screens (Soul, Schedules, Office) and
//! finally Settings + Providers. Plans 06 / 07 / 08 each edit exactly
//! one file per screen — the router itself stays untouched after this
//! plan lands.

use super::screens;
use crate::state::Screen;
use dioxus::prelude::*;

#[component]
pub fn ScreenRouter() -> Element {
    let active = use_context::<Signal<crate::state::Screen>>();
    // Drop the borrow immediately (clippy signal-borrow-safety rule).
    let cur = *active.read();

    rsx! {
        screens::chat::ScreenChat { is_active: cur == Screen::Chat }
        screens::sessions::ScreenSessions { is_active: cur == Screen::Sessions }
        screens::agents::ScreenAgents { is_active: cur == Screen::Agents }
        screens::skills::ScreenSkills { is_active: cur == Screen::Skills }
        screens::models::ScreenModels { is_active: cur == Screen::Models }
        screens::memory::ScreenMemory { is_active: cur == Screen::Memory }
        screens::soul::ScreenSoul { is_active: cur == Screen::Soul }
        screens::tools::ScreenTools { is_active: cur == Screen::Tools }
        screens::schedules::ScreenSchedules { is_active: cur == Screen::Schedules }
        screens::gateway::ScreenGateway { is_active: cur == Screen::Gateway }
        screens::office::ScreenOffice { is_active: cur == Screen::Office }
        screens::settings::ScreenSettings { is_active: cur == Screen::Settings }
        screens::providers::ScreenProviders { is_active: cur == Screen::Providers }
    }
}

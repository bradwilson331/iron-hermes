//! Screen -> short-name map for the header's MODULE segment.
//!
//! Phase 54.4 Plan 01 (D-07): the old top-left breadcrumb chip
//! (`NODE HERMES-7 › BRIDGE › {SCREEN}`) is retired — the persistent header's
//! status strip (`sys_meta.rs`) carries the active screen as its MODULE
//! segment. This file now owns only the pure `Screen` -> short-name map. The
//! map is local to this file (rather than a method on `Screen`) to keep
//! `state.rs` minimal.

use crate::state::Screen;

/// Short-name map for the status strip's MODULE segment. 4-9 char uppercase
/// labels, with the off-wheel screens (`Soul`, `Schedules`, `Office`) defined
/// locally since they are NOT `WheelWedge` variants (D-10). Never empty. The
/// match is exhaustive on purpose: adding a `Screen` variant (plan 07 adds
/// `Logs` => "LOGS") fails to compile until it is named here.
pub(crate) fn screen_short_name(s: Screen) -> &'static str {
    match s {
        Screen::Chat => "CHAT",
        Screen::Sessions => "SESSIONS",
        Screen::Agents => "AGENTS",
        Screen::Skills => "SKILLS",
        Screen::Models => "MODELS",
        Screen::Memory => "MEMORY",
        Screen::Soul => "SOUL",
        Screen::Tools => "TOOLS",
        Screen::Schedules => "SCHED",
        Screen::Gateway => "GATEWAY",
        Screen::Office => "OFFICE",
        Screen::Settings => "SYSTEM",
        Screen::Providers => "PROVIDERS",
        // Phase 36.3.7.11 D-02: Kanban wheel-nav + dashboard label.
        Screen::Kanban => "KANBAN",
        // Phase 46.6 Plan 05 (D-07): Artifacts gallery + sandboxed viewer.
        Screen::Artifacts => "ARTIFACTS",
        Screen::ArtifactViewer => "ARTIFACT",
        // Phase 54.4 Plan 07 (D-08): server / audit / agent-activity logs.
        Screen::Logs => "LOGS",
        // Phase 54.9 Plan 09 (D-13): generated views.
        Screen::OpenUi => "OPENUI",
        // Phase 54.10 Plan 01 (D-01): live boards.
        Screen::Dashboard => "DASHBOARD",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_logs_short_name_is_logs() {
        assert_eq!(screen_short_name(Screen::Logs), "LOGS");
    }
}

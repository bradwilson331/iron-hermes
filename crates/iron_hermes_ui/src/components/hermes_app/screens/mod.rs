//! 14 placeholder screen modules — Wave 3 plans (06, 07, 08) replace
//! the bodies with the real screens, one file per screen, with zero
//! coordination beyond `ScreenRouter`'s mount list. Phase 36.3.7.11
//! Plan 04 added the `kanban` module as a minimal placeholder; Plan 01
//! of that phase will replace its body with the live KanbanBoard.
//!
//! Per RESEARCH Pattern 7 every screen is always mounted; the active
//! one carries the `is-active` class supplied by `ScreenRouter`.

pub mod agents;
pub mod agents_diff;
pub mod chat;
pub mod gateway;
pub mod memory;
pub mod models;
pub mod office;
pub mod providers;
pub mod schedules;
pub mod sessions;
pub mod settings;
pub mod skills;
pub mod soul;
pub mod tools;
// Phase 36.3.7.11 Plan 04 added the `Screen::Kanban` variant + wheel-nav wedge;
// Plan 01 (D-02) supplies the live KanbanBoard + child components (board /
// column / card). Plans 02-03 add drawer + modals + drag-and-drop.
pub mod kanban;

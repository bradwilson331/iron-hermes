//! 14 placeholder screen modules — Wave 3 plans (06, 07, 08) replace
//! the bodies with the real screens, one file per screen, with zero
//! coordination beyond `ScreenRouter`'s mount list. Phase 36.3.7.11
//! Plan 04 added the `kanban` module as a minimal placeholder; Plan 01
//! of that phase will replace its body with the live KanbanBoard.
//!
//! RESEARCH Pattern 7 originally mounted EVERY screen at once, with the active
//! one carrying an `is-active` class. That is no longer true: the Phase 49.4
//! hotfix in `ScreenRouter` renders ONLY the active screen, because mounting all
//! 16 at once ran every screen's fetches, polls, and WebGL loops simultaneously
//! and froze the single-threaded WASM client.
//!
//! Anything that relied on "screen X is always mounted" must therefore not
//! depend on X being rendered. The known instance was CSS: `kanban.css` was
//! reached only via `ScreenKanban`, yet it owns the shared `.kn-modal-*` /
//! `.kn-drawer-*` dialog shell every screen's modals use (see the comments in
//! `components.css` and `bots.css`). It is now linked unconditionally from
//! `app.rs`. Screen-specific CSS may still be linked by its own screen.

pub mod agents;
pub mod agents_diff;
// Phase 50.1 Plan 01 (D-08/D-09): bot roster section mounted by
// `ScreenAgents`, above the pre-existing subagent-turn grid.
pub mod bot_roster;
// Phase 50.1 Plan 02 (D-10): shared profile create wizard, detail drawer and
// switcher — the one implementation consumed by both `kanban.rs` and the
// bot roster / agents screen. The old `screens/kanban/{wizard,
// profile_drawer,profile_switcher}.rs` paths remain as thin re-export shims.
pub mod profile_shared;
// Phase 46.6 Plan 05 (D-07): lean artifacts gallery + sandboxed viewer,
// reached from the Sessions screen's `▤ ARTIFACTS` affordance.
pub mod artifact_viewer;
pub mod artifacts;
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
// Phase 49.4 Plan 07 (D-05..D-09): the IMPORT / NEW SKILL / SKILL.md-editor
// wizard components mounted from `ScreenSkills` (`skills.rs`).
pub mod skills_import;
pub mod soul;
pub mod tools;
// Phase 36.3.7.11 Plan 04 added the `Screen::Kanban` variant + wheel-nav wedge;
// Plan 01 (D-02) supplies the live KanbanBoard + child components (board /
// column / card). Plans 02-03 add drawer + modals + drag-and-drop.
pub mod kanban;

pub mod voice_mode;

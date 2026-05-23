//! 13 placeholder screen modules — Wave 3 plans (06, 07, 08) replace
//! the bodies with the real screens, one file per screen, with zero
//! coordination beyond `ScreenRouter`'s mount list.
//!
//! Per RESEARCH Pattern 7 every screen is always mounted; the active
//! one carries the `is-active` class supplied by `ScreenRouter`.

pub mod chat;
pub mod sessions;
pub mod settings;
pub mod agents;
pub mod agents_diff;
pub mod skills;
pub mod models;
pub mod memory;
pub mod soul;
pub mod tools;
pub mod schedules;
pub mod gateway;
pub mod office;
pub mod providers;

//! Phase 50.1 Plan 02 (D-10): thin re-export shim. `CreateProfileWizard`
//! relocated to `screens/profile_shared/create_dialog.rs` so Kanban and the
//! Agents screen share ONE implementation; this path stays live so
//! `kanban.rs`'s existing import site keeps resolving unchanged.
pub use crate::components::hermes_app::screens::profile_shared::create_dialog::*;

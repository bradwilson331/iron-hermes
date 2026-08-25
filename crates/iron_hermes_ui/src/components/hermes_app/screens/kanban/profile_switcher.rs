//! Phase 50.1 Plan 02 (D-10): thin re-export shim. `ProfileSwitcher`
//! relocated to `screens/profile_shared/switcher.rs` so it lives alongside
//! the other shared profile components; this path stays live so
//! `kanban.rs`'s existing import site keeps resolving unchanged.
pub use crate::components::hermes_app::screens::profile_shared::switcher::*;

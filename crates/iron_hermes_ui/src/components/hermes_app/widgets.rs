//! Phase 50.1 Plan 04 (D-12): shared, reusable presentational widgets for
//! `hermes_app` — the first is [`bot_face`], the deterministic geometric
//! avatar renderer. This module declares NO context provider: every widget
//! here is a pure, prop-driven render component, consumed by callers that
//! already hold whatever state they need. All of this crate's shared
//! context providers live in `hermes_app/mod.rs` (see that file's own
//! discipline note) — a provider scoped to a widget module would compile
//! green and panic its consumers at runtime.

pub mod active_now_strip;
pub mod avatar_picker;
pub mod bot_face;

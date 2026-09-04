//! Re-export shim (Phase 49.5 Plan 05).
//!
//! The automation blueprint catalog previously lived here in full. It moved
//! to `ironhermes_core::blueprint` (Rule 4 escalation, operator-approved) so
//! `cmd_blueprint`'s `list`/`show` verbs — which live in `ironhermes-core` —
//! can read it with zero `CommandContext` handle. `ironhermes-cron` already
//! depends on `ironhermes-core`; the reverse direction is a real cyclic
//! package dependency (verified with a `cargo build`), so `ironhermes-core`
//! cannot depend back on this crate. See `ironhermes_core::blueprint`'s
//! module doc for the full rationale.
//!
//! This re-export keeps every existing external caller
//! (`ironhermes_cron::blueprint::catalog()`, `blueprints_api.rs`,
//! `writer_impl.rs`, the cron-runner) compiling unchanged — the public path
//! `ironhermes_cron::blueprint::*` still resolves to the same items.

pub use ironhermes_core::blueprint::*;

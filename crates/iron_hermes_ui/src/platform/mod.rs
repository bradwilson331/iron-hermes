//! Platform-specific helpers for IronHermes.
//!
//! Phase 4 (per CONTEXT D-04) introduces `timer` for cfg-gated async sleep.
//! Phase 6 will add mobile-shell platform helpers alongside it.

pub mod timer;

//! OAuth token store and flow modules for IronHermes.
//!
//! - `store` — Phase 41 `AuthStore` / `TokenEntry` / `DcrEntry` (moved from `auth.rs`).
//!   Re-exported at this level so `pub use auth::{AuthStore, DcrEntry, TokenEntry}`
//!   in `lib.rs` continues to compile unchanged.
//! - `pkce` — PKCE browser/loopback flow seam (Wave 2, Plan 03 fills the stub).
//! - `device` — RFC 8628 device-code flow seam (Wave 2, Plan 04 fills the stub).
//!
//! D-01: this module lives inside `ironhermes-core`; no `ironhermes-auth` crate is created.

pub mod store;
pub use store::*;

pub mod device;
pub mod pkce;

// Re-export the headless-detection helper so callers can use
// `ironhermes_core::auth::is_headless()` without spelling out the sub-module.
pub use device::is_headless;

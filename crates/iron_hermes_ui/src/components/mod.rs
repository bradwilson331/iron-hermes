// Phase 26.2.1 module layout.
//
// The new wheel-driven shell (`hermes_app`) is the default mount point and is
// always compiled. The pre-26.2.1 Warp-style shell (`shell_legacy` + the
// `warp_hermes` composition root) is preserved verbatim behind the
// `legacy-shell` Cargo feature so we can still run the old UAT flow.

pub mod hermes_app;

#[cfg(feature = "legacy-shell")]
pub mod shell_legacy;

#[cfg(feature = "legacy-shell")]
pub mod warp_hermes;

#[cfg(feature = "legacy-shell")]
pub use warp_hermes::WarpHermes;

// Consumers of legacy primitives import via
// `crate::components::shell_legacy::TitleBar` etc., gated by the feature flag.

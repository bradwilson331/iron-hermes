//! Server-side modules for the Dioxus UI backend.
//!
//! All sub-modules are gated behind `#[cfg(feature = "server")]` so the WASM
//! client binary never compiles server-only code.

#[cfg(feature = "server")]
pub mod api;
#[cfg(feature = "server")]
pub mod ws;
#[cfg(feature = "server")]
pub mod state;

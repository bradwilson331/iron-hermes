//! MIT client for the `ironhermes-tirith` detection sidecar (Phase 57, D-02 / D-12).
//!
//! [`client::SidecarClient`] supervises one long-lived sidecar child over
//! newline-delimited JSON on stdio. Every failure surfaces as a typed
//! [`client::Unavailable`]; no error path is ever reported as a clean (empty)
//! finding list.
//!
//! This crate is MIT and never links `tirith-core`: it only knows the wire types
//! in `ironhermes-security-proto` and the path of the AGPL sidecar binary.

pub mod client;
pub mod locate;
pub mod policy;
pub mod service;
pub mod testing;

pub use client::{
    SidecarClient, SidecarHealth, SidecarOptions, SidecarState, Unavailable, UnavailableReason,
    backoff_after_healthy, next_backoff,
};
pub use ironhermes_security_proto::{SidecarVersion, SubjectKind, WireFinding, WireSeverity};
pub use locate::{SIDECAR_BIN_NAME, SIDECAR_ENV_OVERRIDE, locate_from, locate_sidecar_binary};

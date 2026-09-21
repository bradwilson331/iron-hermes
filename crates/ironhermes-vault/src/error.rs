//! [`VaultError`] — the error type shared by every `SecretStore` backend (Phase 46.8 D-01/D-07).
//!
//! D-07: a sealed or uninitialized vault backend hard-errors rather than silently falling
//! back — callers must run an explicit `ironhermes vault unlock`/`vault init` before secret
//! traffic flows.

use thiserror::Error;

/// Errors returned by any `SecretStore` backend implementation.
#[derive(Debug, Error)]
pub enum VaultError {
    /// The backend is initialized but currently sealed. Message names the remediation
    /// command per D-07's hard-error posture.
    #[error("vault is sealed — run `ironhermes vault unlock`")]
    Sealed,

    /// The backend has never been initialized (no keyfile/data dir present yet).
    #[error("vault is not initialized — run `ironhermes vault init`")]
    NotInitialized,

    /// An I/O error occurred while reading/writing backend state.
    #[error("vault I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A backend-specific error, opaque to callers beyond the message.
    #[error("vault backend error: {0}")]
    Backend(String),

    /// The requested backend was compiled out (e.g. `rusty-vault` feature not enabled).
    #[error("vault backend unavailable — the `rusty-vault` feature was not compiled in")]
    BackendUnavailable,

    /// T-46.8-16 (46.8-gap): a secret key name failed validation — empty, contains a
    /// path separator (`/`), a parent-directory traversal token (`..`), or a control
    /// character. Never carries the offending key's context beyond what's already a
    /// caller-supplied identifier (never a secret VALUE, D-08/D-15 safe to format).
    #[error("invalid vault key: {0}")]
    InvalidKey(String),

    /// Phase 51 Task 3 (D-08 layer 2, T-51-51): a request was authenticated against a real
    /// token, but access was refused — either the presented token is unrecognized
    /// (`rusty_vault`'s own `TokenStore::check_token`) or a recognized token's policies do
    /// not grant the requested path (`PolicyStore::post_auth`'s ACL evaluation). Both surface
    /// as the identical `RvError::ErrPermissionDenied` at the pinned rev, so this one variant
    /// covers both — the message never reveals which case fired, and never the presented
    /// token. Distinguishable from [`VaultError::Sealed`]/[`VaultError::NotInitialized`]
    /// ("unreachable") and from `Ok(None)` (absent leaf, the one legitimately empty outcome).
    #[error("vault request denied — token unrecognized or policy does not grant this operation")]
    Denied,

    /// Reserved for Plan 03 (D-15): the caller's profile-scoped token has expired. This plan
    /// (51-09) never constructs this variant — Plan 03's token-minting/TTL work wires it in on
    /// top of the same taxonomy, so "unreachable" ([`VaultError::Sealed`] /
    /// [`VaultError::NotInitialized`]), "denied" ([`VaultError::Denied`]), and "expired"
    /// (this variant) stay three outcomes distinguishable by variant rather than expiry being
    /// folded into `Denied`.
    #[error("vault token expired — request a new profile-scoped token")]
    TokenExpired,
}

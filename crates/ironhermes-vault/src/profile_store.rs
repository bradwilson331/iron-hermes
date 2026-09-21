//! [`ProfileSecretStore`] — the additive, profile-scoped `SecretStore` sibling (Phase 51
//! D-04/D-06/D-08).
//!
//! # Additive, not a `SecretStore` widening (D-06 `add-alongside`)
//!
//! [`crate::SecretStore`] keeps its exact four `key: &str` methods and `open_store`'s
//! signature does not move — `crates/ironhermes-agent/tests/invariants_41_3_credentials.rs`
//! pins that literal call shape across roughly fifteen production call sites. The secret's
//! identity generalizes from `provider` (one flat keyspace) to `(scope, provider)`, but that
//! generalization arrives as a SIBLING type with its own methods, never as a trait edit — see
//! this phase's `assumption_delta_decision` in `51-09-PLAN.md` for the full ruling.
//!
//! # Two separately-validated parameters, never a pre-joined path (D-08 layer 1, T-51-50/50b)
//!
//! Every method here takes `slug` and `leaf` as two separate `&str` parameters and asks
//! [`crate::profile_paths::profile_secret_path`] / [`crate::profile_paths::profile_secret_prefix`]
//! to validate and join them — there is never an intermediate value in which a
//! traversal-bearing compound path exists as a string. This module declares no validator, no
//! character-class check, and no local `format!` of the address; it imports Plan 02's shared
//! validators exactly once, through those two functions, and nowhere else.
//!
//! # Reused request plumbing, not a second copy (D-04)
//!
//! [`crate::rusty_vault_store::read_request`]/`write_request`/`delete_request`/`list_request`
//! and [`crate::rusty_vault_store::run_request`] are already generalized over BOTH the path and
//! the token — this module reuses them verbatim (elevated to `pub(crate)` for this purpose)
//! rather than declaring a second, drift-prone request builder. The one place this module
//! deliberately does NOT reuse existing plumbing is the list root: [`crate::SecretStore::list_secrets`]'s
//! list request is rooted at the fixed `secret/providers/` constant and its `prefix` argument
//! is only a `starts_with` filter over names that root returned, so no argument can make it
//! select `secret/profiles/` — [`ProfileSecretStore::list_profile_secret_names`] issues a FRESH
//! list request rooted at [`crate::profile_paths::profile_secret_prefix`] instead.
//!
//! # Three distinguishable outcomes, not two (D-08 layer 2, Phase 51 Task 3)
//!
//! Every method that can fail distinguishes exactly three outcomes, by error VARIANT rather
//! than by message substring:
//!
//! - **Unreachable** — the vault is sealed, uninitialized, or otherwise unopenable:
//!   [`crate::error::VaultError::Sealed`] / [`crate::error::VaultError::NotInitialized`].
//! - **Denied** — the request was authenticated but refused, either because the presented
//!   token is unrecognized (`rusty_vault`'s `TokenStore::check_token`) or because a recognized
//!   token's policies do not grant the path (`PolicyStore::post_auth`'s ACL evaluation). Both
//!   surface as the identical `RvError::ErrPermissionDenied` at the pinned rev (confirmed by
//!   direct read of `modules/auth/token_store.rs:377-378` and
//!   `modules/policy/policy_store.rs:671-672`), so this crate's [`crate::error::VaultError::Denied`]
//!   covers both.
//! - **Absent** — the path is reachable and authorized, and the leaf simply is not there:
//!   `Ok(None)`. This is the ONLY outcome that may legitimately be empty.
//!
//! Plan 03 (D-15) adds a fourth outcome — a caller's profile-scoped token has expired — on top
//! of this same taxonomy via [`crate::error::VaultError::TokenExpired`], reserved here rather
//! than folded into `Denied`.
//!
//! # `read_profile_secret_with_token` cannot reach the root token (D-08 layer 2, D-11, T-51-51)
//!
//! [`ProfileSecretStore::read_profile_secret_with_token`] takes the caller's token as a
//! parameter and its body — and everything it calls on that path — never reads
//! `self.root_token`. This is structural, not documented-only: a read that quietly fell back to
//! the root token would make the native ACL decorative for the only caller that matters (the
//! kanban worker path, D-11), which is the exact failure mode D-11 rejected the
//! dispatcher-injects-the-secret shape to avoid.

use std::sync::Arc;

use rusty_vault::core::Core;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value as JsonValue;

use crate::error::VaultError;
use crate::profile_paths::{profile_secret_path, profile_secret_prefix};
use crate::rusty_vault_store::{
    RustyVaultStore, delete_request, list_request, read_request, run_request, write_request,
};

/// Additive profile-scoped `SecretStore` sibling — see the module doc for why this is a new
/// type rather than a widened [`crate::SecretStore`] trait (D-06 `add-alongside`).
pub struct ProfileSecretStore {
    core: Arc<Core>,
    /// Root token — read ONLY by the four root-authorized methods below. Never read from
    /// [`ProfileSecretStore::read_profile_secret_with_token`]'s body or anything it calls; that
    /// structural separation is D-08 layer 2's entire point.
    root_token: SecretString,
}

impl ProfileSecretStore {
    /// Build a profile-scoped handle over the SAME `Core` an already-open [`RustyVaultStore`]
    /// holds, rather than opening a second `Core` against the same data dir.
    pub fn from_rusty_vault_store(store: &RustyVaultStore) -> Self {
        Self {
            core: store.core_handle(),
            root_token: store.root_token_secret(),
        }
    }

    /// Crate-internal constructor for a caller that already holds an `Arc<Core>` directly,
    /// rather than a [`RustyVaultStore`] wrapping one (Phase 51 Plan 06, `profile_endpoint.rs`).
    /// `root_token` is required by this struct's root-authorized methods but is NEVER read by
    /// [`ProfileSecretStore::read_profile_secret_with_token`] — see the module doc's "cannot
    /// reach the root token" section — so the credential endpoint's runtime read path (the only
    /// method it calls) is unaffected by which token this parameter carries. This mirrors the
    /// established pattern in this phase's own test fixtures (`profile_token_mint.rs`'s
    /// `new_core` helper), which build a raw `Core` directly because `RustyVaultStore`'s root
    /// token is `pub(crate)`-only and not obtainable from outside this crate.
    pub(crate) fn from_core(core: Arc<Core>, root_token: SecretString) -> Self {
        Self { core, root_token }
    }

    /// Write (create or overwrite) a profile-scoped secret at `secret/profiles/{slug}/{leaf}`,
    /// authorized as root. `slug` and `leaf` are validated independently — see the module doc.
    pub async fn put_profile_secret(
        &self,
        slug: &str,
        leaf: &str,
        value: SecretString,
    ) -> Result<(), VaultError> {
        let path = profile_secret_path(slug, leaf)?;
        let token = self.root_token.expose_secret().to_string();
        // D-08 boundary: expose only to build the request body handed to the vault backend —
        // never into a log/Debug/error string.
        let raw_value = value.expose_secret().to_string();
        let core = Arc::clone(&self.core);

        run_request(core, move || write_request(&path, &raw_value, &token)).await?;
        Ok(())
    }

    /// Read a profile-scoped secret, authorized as root. `Ok(None)` for an absent leaf — the
    /// one legitimately empty outcome (see the module doc's three-outcome taxonomy).
    pub async fn get_profile_secret_as_root(
        &self,
        slug: &str,
        leaf: &str,
    ) -> Result<Option<SecretString>, VaultError> {
        let path = profile_secret_path(slug, leaf)?;
        let token = self.root_token.expose_secret().to_string();
        let core = Arc::clone(&self.core);

        let data = run_request(core, move || read_request(&path, &token)).await?;
        decode_value(data)
    }

    /// Delete a profile-scoped secret, authorized as root. Deleting an absent leaf is a no-op
    /// `Ok(())`, matching `rusty_vault_store.rs`'s root-keyspace behavior (the physical `file`
    /// backend already treats a missing key as `Ok(())` on delete).
    pub async fn delete_profile_secret(&self, slug: &str, leaf: &str) -> Result<(), VaultError> {
        let path = profile_secret_path(slug, leaf)?;
        let token = self.root_token.expose_secret().to_string();
        let core = Arc::clone(&self.core);

        run_request(core, move || delete_request(&path, &token)).await?;
        Ok(())
    }

    /// List the leaf NAMES written under `slug`'s own prefix, authorized as root. Names only,
    /// never values (D-15/T-51-53).
    ///
    /// Issues a FRESH list request rooted at [`profile_secret_prefix`] — never through
    /// [`crate::SecretStore::list_secrets`], whose list request is rooted at the fixed
    /// `secret/providers/` constant and whose `prefix` argument is only a `starts_with` filter
    /// over names THAT root already returned, so no argument to that method can ever select
    /// `secret/profiles/` (this is the top blocker the cross-AI review found — see the Task 1
    /// RED commit for the observed failure this exact defect produces).
    pub async fn list_profile_secret_names(&self, slug: &str) -> Result<Vec<String>, VaultError> {
        let prefix = profile_secret_prefix(slug)?;
        let token = self.root_token.expose_secret().to_string();
        let core = Arc::clone(&self.core);

        let data = run_request(core, move || list_request(&prefix, &token)).await?;
        let Some(data) = data else {
            return Ok(Vec::new());
        };
        let keys = data
            .get("keys")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| VaultError::Backend("kv list missing \"keys\" field".to_string()))?;
        let mut names: Vec<String> = keys
            .iter()
            .filter_map(JsonValue::as_str)
            .map(str::to_string)
            .collect();
        names.sort();
        Ok(names)
    }

    /// Read a profile-scoped secret authorized under the CALLER's own token — never the root
    /// token (D-08 layer 2, D-11's worker path, T-51-51). This method's body — and everything
    /// it calls on this path — never reads `self.root_token`; the native ACL evaluates `token`
    /// exactly as it would for any other caller.
    ///
    /// Four distinguishable outcomes, three of them implemented here:
    /// - **Unreachable** — [`VaultError::Sealed`] / [`VaultError::NotInitialized`]: the vault
    ///   is sealed, uninitialized, or otherwise unopenable.
    /// - **Denied** — [`VaultError::Denied`]: the presented token is unrecognized, or a
    ///   recognized token's policies do not grant this path.
    /// - **Absent** — `Ok(None)`: the path is reachable and authorized, and the leaf simply is
    ///   not there. This is the ONLY outcome that may legitimately be empty.
    /// - **Expired** — [`VaultError::TokenExpired`]: RESERVED for Plan 03 (D-15). Not
    ///   constructed by this method today; Plan 03's token-minting/TTL work extends this same
    ///   taxonomy by wiring it in, rather than folding expiry into `Denied`.
    pub async fn read_profile_secret_with_token(
        &self,
        token: &SecretString,
        slug: &str,
        leaf: &str,
    ) -> Result<Option<SecretString>, VaultError> {
        let path = profile_secret_path(slug, leaf)?;
        let caller_token = token.expose_secret().to_string();
        let core = Arc::clone(&self.core);

        let data = run_request(core, move || read_request(&path, &caller_token)).await?;
        decode_value(data)
    }
}

/// Shared response decoding for the two read paths — pulls the `"value"` field out of the KV
/// entry's data map, matching `rusty_vault_store.rs::get_secret`'s exact shape.
///
/// `pub(crate)` (Phase 51 Plan 17, IN-03): `profile_token.rs::read_profile_secret_with_minted_token`
/// reuses this exact fn rather than keeping its own byte-identical `decode_secret_value` copy —
/// that duplicate existed only because an earlier plan's `files_modified` was scoped narrowly
/// enough to avoid touching this file; the plan that imposed that constraint is done.
pub(crate) fn decode_value(
    data: Option<serde_json::Map<String, JsonValue>>,
) -> Result<Option<SecretString>, VaultError> {
    let Some(data) = data else {
        return Ok(None);
    };
    let value = data
        .get("value")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| VaultError::Backend("kv entry missing \"value\" field".to_string()))?;
    Ok(Some(SecretString::from(value.to_string())))
}

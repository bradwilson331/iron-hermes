//! Per-profile RustyVault ACL policy generation and registration (Phase 51 D-05/D-07).
//!
//! [`render_profile_policy`] builds the D-05 two-block HCL text directly from
//! [`crate::profile_paths::profile_secret_prefix`] — never from a literal path of its own — so
//! the prefix a profile is granted and the prefix a profile actually reads can never drift
//! apart. [`ensure_profile_policy`] registers that text under this crate's `profile-{slug}`
//! naming convention through RustyVault's real policy-write entry point at the pinned rev
//! (`sys/policy/<name>`, the exact path `PolicyModule::handle_policy_write` is wired to via
//! `SystemModule`'s route table, and the same shape the crate's own `test_write_api` test
//! helper uses — verified by direct read, not assumed), idempotently: RustyVault's own
//! `PolicyStore::set_policy` keys by policy name and overwrites on write, so a second call for
//! the same slug leaves exactly one registered policy rather than duplicating it.

use std::sync::Arc;

use rusty_vault::core::Core;
use rusty_vault::errors::RvError;
use rusty_vault::logical::{Operation, Request};
use serde_json::json;

use crate::error::VaultError;
use crate::profile_paths::{profile_policy_name, profile_secret_prefix};

/// Map a `rusty_vault` error onto [`VaultError`], matching `rusty_vault_store.rs`'s own
/// hard-error-don't-mask posture (D-07/D-14) rather than swallowing a sealed or uninitialized
/// vault as a generic failure.
fn map_rv_error(e: RvError) -> VaultError {
    match e {
        RvError::ErrBarrierSealed => VaultError::Sealed,
        RvError::ErrBarrierNotInit => VaultError::NotInitialized,
        other => VaultError::Backend(other.to_string()),
    }
}

/// Render the D-05 two-block ACL policy for `slug`: the profile's own subtree grants
/// `read`+`list`, and the broader profile-root glob is denied (cheap insurance — `deny`
/// participates in precedence, so a future policy granting a wide glob still loses to this
/// one on the profile's own subtree). Both blocks are derived from [`profile_secret_prefix`]
/// rather than a literal of this function's own, so the granted prefix and the runtime read
/// path can never disagree.
pub fn render_profile_policy(slug: &str) -> Result<String, VaultError> {
    let own_prefix = profile_secret_prefix(slug)?;
    let slug_segment = format!("{slug}/");
    let broad_prefix = own_prefix.strip_suffix(slug_segment.as_str()).ok_or_else(|| {
        VaultError::Backend(
            "profile prefix builder changed shape — expected it to end with the slug segment"
                .to_string(),
        )
    })?;

    Ok(format!(
        "path \"{own_prefix}*\" {{\n  capabilities = [\"read\", \"list\"]\n}}\n\npath \"{broad_prefix}*\" {{\n  capabilities = [\"deny\"]\n}}\n"
    ))
}

/// Register (or re-register) `slug`'s rendered policy under `profile-{slug}` through
/// RustyVault's real `sys/policy/<name>` write path, using `root_token` as the request's
/// client token — the same construction the crate's own `test_write_api` test helper uses.
/// D-07 calls this on every dispatch, so idempotency matters: two calls for the same slug
/// leave exactly one registered policy and both return `Ok`.
pub async fn ensure_profile_policy(
    core: &Arc<Core>,
    root_token: &str,
    slug: &str,
) -> Result<(), VaultError> {
    let policy_name = profile_policy_name(slug)?;
    let policy_text = render_profile_policy(slug)?;

    let mut req = Request::new(format!("sys/policy/{policy_name}"));
    req.operation = Operation::Write;
    req.client_token = root_token.to_string();
    req.body = Some(
        json!({ "policy": policy_text })
            .as_object()
            .expect("json object literal is always a map")
            .clone(),
    );

    core.handle_request(&mut req).await.map_err(map_rv_error)?;
    Ok(())
}

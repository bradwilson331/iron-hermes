//! Layer 3 of D-08's three independent enforcement layers (Phase 51 D-08/D-09): a `post_auth`
//! `AuthHandler` that derives its allowed prefix from the REQUEST's own resolved token
//! policies — never a parameter, never ambient state — and denies a cross-profile read even
//! when the native ACL (layer 2) would allow it.
//!
//! # Why `post_auth`, not `pre_route` (the cross-AI review's second blocker, corrected)
//!
//! The pre-review design specified a Core-level `Handler::pre_route` guard. Verified directly
//! against the pinned rev (`0922e6d5e6fbe6fd2d909863eca6d583623f4ad7`), that placement is not
//! simply wrong — it is right or wrong depending on registration timing the original design
//! never pinned, which is worse: `TokenStore::pre_route`
//! (`src/modules/auth/token_store.rs:772-816`) is the ONLY place `req.auth` is ever assigned —
//! it calls `check_token`, sets `req.auth = auth`, and only THEN runs the `auth_handlers`'
//! `post_auth` loop. A `Handler::pre_route` guard registered on `Core` BEFORE `TokenStore`'s
//! own `pre_route` runs in the SAME outer `handle_pre_route_phase` iteration (i.e. registered
//! before unseal, since `AuthModule::init` appends `TokenStore` to `Core::handlers` during
//! unseal via `core.add_handler(ts)`, `src/modules/auth/mod.rs:352`) observes `req.auth` as
//! `None` — it has nothing to check against and, unless painstakingly written to deny on
//! absent data (a case easy to overlook, since a `pre_route` author would reasonably expect
//! `TokenStore` to have already run), silently passes every request through. Registered AFTER
//! unseal, the exact same code WOULD see `req.auth` populated, because `handle_pre_route_phase`
//! iterates all handlers' `pre_route` in one pass and `TokenStore`'s own `pre_route` already
//! ran earlier in that pass. A guard whose correctness depends on unstated registration timing
//! enforces or not by accident (T-51-17c) — strictly worse than a guard that is flatly wrong,
//! because it can pass every test built against it and still be inert in production if wired a
//! few lines differently.
//!
//! `AuthHandler::post_auth` removes this dependency entirely: at the pinned rev it is invoked
//! from INSIDE `TokenStore::pre_route`, after `req.auth` is already assigned, in the very same
//! function call — there is no "before" or "after" TokenStore for a `post_auth` implementation
//! to land on either side of. `guard_sees_the_resolved_policy_set_at_post_auth` proves the data
//! is there; the grep gates on this file assert no `Handler`/`pre_route` impl exists; Task 3's
//! `pre_route_before_unseal_sees_no_policies` / `pre_route_after_unseal_sees_policies` pair
//! demonstrates the rejected design's timing-dependent blindness directly, and
//! `handler_registration_order_is_observed_not_assumed` pins the vector positions this
//! reasoning depends on as an OBSERVED fact, not an inferred one.
//!
//! # Registration ordering facts (Task 3), tied to rev `0922e6d5e6fbe6fd2d909863eca6d583623f4ad7`
//!
//! Observed directly (`handler_registration_order_is_observed_not_assumed` asserts every claim
//! below against a real `Core`, not merely by reading source):
//!
//! - `Core::new`/`Core::default` seed `handlers` with the router ALONE
//!   (`handlers: ArcSwap::from_pointee(vec![router])`, `src/core.rs:117,135`).
//! - `Router`'s own `Handler` impl (`src/router.rs:251-259`) declares only `name` and `route` —
//!   it has NO `pre_route` override, so it is never a source of `req.auth` visibility either
//!   way.
//! - `AuthModule::init` appends `TokenStore` to `handlers` via `core.add_handler(ts)` at the
//!   END of its own init sequence (`src/modules/auth/mod.rs:352`), which itself runs from
//!   `Core::post_unseal`'s `self.module_manager.init(self).await?` — i.e. on `Core::unseal`,
//!   not on `Core::new`/`Core::init`. Before the first successful `unseal()`, `handlers` is
//!   `[router]`; after, `[router, token_store]`.
//! - `Core::add_handler`/`Core::add_auth_handler` both APPEND (`src/core.rs:257-267,277-291`)
//!   — a handler registered after unseal lands strictly after `TokenStore` in `handlers`, and
//!   an auth handler registered after unseal lands strictly after `PolicyStore` in
//!   `auth_handlers`.
//! - `PolicyModule::init` registers `PolicyStore` as an `auth_handlers` entry
//!   (`src/modules/policy/mod.rs:147`, `core.add_auth_handler(policy_store)`) during the SAME
//!   `module_manager.init` pass `AuthModule::init` runs in. This crate's own
//!   [`register_profile_guard`] therefore always lands AFTER `policy_store`:
//!   `auth_handlers == [policy_store, profile_guard]`.
//! - This ordering is why Run A's empty guard sink (D-09, `tests/profile_layer_independence.rs`)
//!   is STRUCTURAL rather than incidental: `TokenStore::pre_route`'s `post_auth` loop
//!   (`src/modules/auth/token_store.rs:800-816`) returns on the FIRST non-`ErrHandlerDefault`
//!   error — with the correct D-05 policy in force, `PolicyStore::post_auth` denies a
//!   cross-profile read and returns first, so this guard's `post_auth` is never even invoked.
//!   Run A cannot pass because this guard quietly does the work; it is not reached.
//!
//! # No continue-on-error branch
//!
//! Every internal error path in [`ProfileGuard::post_auth`] returns
//! `Err(RvError::ErrPermissionDenied)`. `Err(RvError::ErrHandlerDefault)` — the chain's
//! "not handled, continue" signal — is returned from exactly two places: the request path is
//! outside the profile namespace, or it is inside the token's OWN derived prefix. There is no
//! `?`-propagation or fallback anywhere in this file that could turn an internal failure into a
//! continue signal; a future edit that adds one should read as obviously wrong against this
//! statement.
//!
//! # `is_unauth_path` limitation
//!
//! `TokenStore::pre_route` returns `Ok(None)` immediately for any `is_unauth_path` request
//! (`src/modules/auth/token_store.rs:773-776`) — BEFORE the `auth_handlers` `post_auth` loop
//! this guard lives in ever runs. Neither this guard NOR `PolicyStore`'s own ACL check runs
//! for `sys/init`, `sys/unseal`, or `sys/seal-status`. This is not a gap in this guard; it is
//! the concrete reason D-12 forbids serving RustyVault's own HTTP module at all — those three
//! paths are out of reach of every policy-based layer this phase builds, by design of the
//! dependency itself, not by an oversight here.

use std::sync::Arc;

use rusty_vault::core::Core;
use rusty_vault::errors::RvError;
use rusty_vault::handler::AuthHandler;
use rusty_vault::logical::Request;

use crate::error::VaultError;
use crate::profile_paths::profile_secret_prefix;
use crate::rusty_vault_store::map_rv_error;

/// The name this handler registers under on `Core::auth_handlers` — stable, so
/// `Core::delete_auth_handler` (which matches by name) reliably removes exactly this handler
/// and no other.
const PROFILE_GUARD_HANDLER_NAME: &str = "ironhermes_profile_guard";

/// The naming convention Plan 02's [`crate::profile_paths::profile_policy_name`] establishes —
/// this guard derives its scope from a resolved policy name matching this prefix, never from a
/// parameter its caller supplies.
const PROFILE_POLICY_PREFIX: &str = "profile-";

/// Structural audit sink for this guard's denial records — a REQUIRED parameter of
/// [`ProfileGuard::new`], mirroring [`crate::profile_token::ProfileTokenAudit`]'s cycle-break
/// shape so a guard constructed without one does not compile.
pub trait ProfileGuardAudit: Send + Sync {
    /// Record a DENIED attempt: the profile slug (when derivable), the requested path, and the
    /// policy name the slug was derived from (when derivable). NEVER the client token, the
    /// secret value, or the response body — this method's signature has no parameter that
    /// could carry any of those (Phase 47.4 CR-05/CR-06 reflex, applied preemptively).
    fn record_denial(&self, slug: &str, path: &str, policy_name: &str) -> anyhow::Result<()>;
}

/// Layer 3 (D-08): an `AuthHandler::post_auth` implementation. See the module doc for why this
/// is an `AuthHandler`, never a `Handler` with `pre_route`.
pub struct ProfileGuard {
    sink: Arc<dyn ProfileGuardAudit>,
}

impl ProfileGuard {
    /// The audit sink is REQUIRED, not `Option` — an un-audited guard does not compile.
    /// Returns an `Arc` directly since every caller needs a `Arc<dyn AuthHandler>`-compatible
    /// handle to pass to [`register_profile_guard`]/[`unregister_profile_guard`] anyway.
    pub fn new(sink: Arc<dyn ProfileGuardAudit>) -> Arc<Self> {
        Arc::new(Self { sink })
    }
}

#[async_trait::async_trait]
impl AuthHandler for ProfileGuard {
    fn name(&self) -> String {
        PROFILE_GUARD_HANDLER_NAME.to_string()
    }

    async fn post_auth(&self, req: &mut Request) -> Result<(), RvError> {
        // Non-profile traffic is untouched — this MUST stay a no-op for root-level
        // secret/providers/* traffic (Plan 01's existing green suite) and every other mount
        // this Core might ever serve. Derived at runtime from profile_secret_prefix rather
        // than a literal of this file's own, so this file never contains the mount-prefix
        // substring (mechanically enforced by this plan's grep gate).
        let Ok(profile_mount) = profile_mount_prefix() else {
            // Unreachable in practice ("probe" is a valid slug literal) — fail closed rather
            // than panic if the prefix builder's shape ever changed underneath this file.
            return Err(RvError::ErrPermissionDenied);
        };
        if !req.path.as_str().starts_with(profile_mount.as_str()) {
            return Err(RvError::ErrHandlerDefault);
        }

        // Fail closed: no resolved auth on the request at all. Unreachable through the normal
        // TokenStore::pre_route flow (which returns ErrPermissionDenied itself before this
        // handler's post_auth would ever run without a populated req.auth) — asserted directly
        // by guard_denies_when_req_auth_is_absent as defense in depth, not dead code.
        let Some(auth) = req.auth.as_ref() else {
            return Err(RvError::ErrPermissionDenied);
        };

        // Derive the slug from the resolved policy set: exactly one `profile-`-prefixed policy
        // is the only valid shape. Zero, two-or-more, or one that fails
        // profile_secret_prefix's own validation all deny.
        let mut profile_policies = auth
            .policies
            .iter()
            .filter(|p| p.starts_with(PROFILE_POLICY_PREFIX));
        let Some(policy_name) = profile_policies.next() else {
            return Err(RvError::ErrPermissionDenied);
        };
        if profile_policies.next().is_some() {
            return Err(RvError::ErrPermissionDenied);
        }
        // Own the values here — breaks any borrow tie to req.auth before req.path is touched
        // again below, and keeps the denial record's inputs unambiguous.
        let policy_name = policy_name.clone();
        let slug = policy_name[PROFILE_POLICY_PREFIX.len()..].to_string();

        let Ok(allowed_prefix) = profile_secret_prefix(&slug) else {
            return Err(RvError::ErrPermissionDenied);
        };

        if req.path.as_str().starts_with(allowed_prefix.as_str()) {
            // Inside the token's own subtree — continue the chain. This guard is an
            // additional layer, not a replacement for layer 2's own evaluation.
            return Err(RvError::ErrHandlerDefault);
        }

        // Outside the token's own subtree — deny and record. The sink call's own failure
        // (e.g. a full trajectory ledger) does not weaken the denial itself; the request is
        // refused either way.
        let _ = self.sink.record_denial(&slug, req.path.as_str(), &policy_name);
        Err(RvError::ErrPermissionDenied)
    }
}

/// The broad `secret/profiles/` mount prefix, derived at runtime from
/// [`profile_secret_prefix`] rather than a literal of this file's own — mirrors
/// `profile_policy.rs::render_profile_policy`'s own `strip_suffix` technique.
fn profile_mount_prefix() -> Result<String, VaultError> {
    let probe = profile_secret_prefix("probe")?;
    probe.strip_suffix("probe/").map(str::to_string).ok_or_else(|| {
        VaultError::Backend(
            "profile prefix builder changed shape — expected it to end with the slug segment"
                .to_string(),
        )
    })
}

/// Register [`ProfileGuard`] on an ALREADY-UNSEALED `core`. `Core::add_auth_handler` reaches
/// `AuthModule::set_auth_handlers`, which unwraps a `token_store` that stays empty until
/// `AuthModule::init` runs during unseal (`src/modules/auth/mod.rs:79`) — calling this before
/// unseal panics rather than failing. Returns the constructed handle so the caller can later
/// pass it to [`unregister_profile_guard`] (`Core::delete_auth_handler` matches by name, but a
/// live handle is the natural way to carry the sink alongside it).
pub fn register_profile_guard(
    core: &Arc<Core>,
    sink: Arc<dyn ProfileGuardAudit>,
) -> Result<Arc<ProfileGuard>, VaultError> {
    let guard = ProfileGuard::new(sink);
    core.add_auth_handler(Arc::clone(&guard) as Arc<dyn AuthHandler>)
        .map_err(map_rv_error)?;
    Ok(guard)
}

/// Remove a previously-registered [`ProfileGuard`] via `Core::delete_auth_handler`, which
/// propagates to the live `TokenStore` through `AuthModule::set_auth_handlers`
/// (`src/modules/auth/mod.rs:79`) — the toggle reaches the live enforcement path, it is not
/// cosmetic (D-09).
pub fn unregister_profile_guard(
    core: &Arc<Core>,
    guard: &Arc<ProfileGuard>,
) -> Result<(), VaultError> {
    core.delete_auth_handler(Arc::clone(guard) as Arc<dyn AuthHandler>)
        .map_err(map_rv_error)?;
    Ok(())
}

//! Mint a short-lived, single-policy, non-renewable profile-scoped token and prove its scope
//! against a real `Core` (Phase 51 D-07/D-08/D-15).
//!
//! # D-07: the runtime mints, the agent never does
//!
//! [`mint_profile_token`] is the only mint entry point. It takes the `Core`, the ROOT token
//! (to authorize the mint — never returned, never logged, never embedded in the minted
//! result), the profile slug, a TTL, and a REQUIRED audit sink — five parameters, no
//! `Option`, no defaulting wrapper. An un-audited mint does not compile.
//!
//! # Corrected finding: `no_default_policy` is not a real request field at this pinned rev
//!
//! D-07's text names `no_default_policy` as part of the mint request. Read directly against
//! the vendored source at the pinned rev
//! (`~/.cargo/git/checkouts/rustyvault-*/0922e6d/src/modules/auth/token_store.rs`),
//! `TokenReqData` (the struct `TokenStore::handle_create` deserializes `auth/token/create`'s
//! JSON body into) has NO `no_default_policy` field at all — only `Auth` (a different type,
//! used by the login/auth-mount path) carries that field, and nothing in `auth/token/create`'s
//! request schema can set it. The mechanism that actually keeps our EXPLICIT policy list from
//! being widened inside `handle_create` itself is structural instead:
//! `sanitize_policies(&mut data.policies, false)` (which is what would inject `"default"`) only
//! runs when `data.policies.is_empty()`. Passing `policies: ["profile-{slug}"]` explicitly
//! skips that branch entirely, so `handle_create`'s own logic never widens our list.
//!
//! # Second corrected finding: `Core`'s generic post-route ALWAYS re-attaches `"default"`
//!
//! `TokenStore` is registered as a global `Handler` on `Core` (`modules/auth/mod.rs:352`,
//! `core.add_handler(ts)`), so ANY request whose response carries an `Auth` — including
//! `auth/token/create`'s own response — passes back through `TokenStore::post_route`
//! (`token_store.rs:815-919`) before `Core::handle_request` returns it. That generic
//! finalization unconditionally runs
//! `sanitize_policies(&mut auth.token_policies, !auth.no_default_policy)`, and since
//! `auth.no_default_policy` can never be set to `true` via `auth/token/create`'s request
//! schema (see above), this ALWAYS adds `"default"` to the policy list that gets persisted
//! into the SECOND, final `TokenEntry` `post_route` creates and registers (overwriting the
//! FIRST entry `handle_create` made internally — `post_route` reassigns
//! `auth.client_token` to the new entry's id before the response returns to the caller).
//! There is no way to suppress this via the standard, sanctioned `Core::handle_request` path;
//! bypassing it (calling `TokenStore::handle_create` directly) was considered and rejected —
//! it would ALSO skip `self.expiration.register_auth(&te, auth)`, meaning the token's lease
//! would never be tracked by the expiration sweep and would never be revocable by TTL at all,
//! trading one cosmetic issue for a real one.
//!
//! **Why this is provably harmless.** `DEFAULT_POLICY`
//! (`modules/policy/policy_store.rs:59-90`, read verbatim) grants exactly:
//! `auth/token/lookup-self` (read), `auth/token/renew-self` (update),
//! `auth/token/revoke-self` (update), `sys/capabilities-self` (update), and two
//! `identity/entity/*` self-lookup paths. NONE of these touch `secret/profiles/*` or
//! `secret/providers/*` — this crate's entire address space. So the token's REAL, persisted
//! policy set is `{"profile-{slug}", "default"}`, not the single-element set D-07's text
//! describes literally — but the ADDITIONAL member grants zero secret-namespace capability,
//! proven two ways in `tests/profile_token_mint.rs`: (a) `default_policy_grants_no_secret_access`
//! builds a real `ACL` from `DEFAULT_POLICY`'s own raw text and asserts it denies both
//! `secret/profiles/alpha/*` and `secret/providers/*`, independent of any code in this crate;
//! (b) the tracer test's own sibling-read/root-namespace-read denials are themselves live
//! proof that `"default"` leaked no extra capability into the running `Core` — if it had, one
//! of those two assertions would fail. T-51-09 (the elevation-of-privilege threat this exists
//! to close) is about CAPABILITY, not about a name appearing in a list; both are checked here.
//!
//! # Third corrected finding: `TokenEntry.ttl` is whole-second granularity, not the `Duration`
//!
//! `calculate_ttl` (`logical/lease.rs:66-134`) truncates the current time to whole seconds and
//! receives `backend_ttl` as a `Duration` built from `te.ttl: u64` (already whole seconds —
//! `handle_create` does `te.ttl = parse_duration(&data.ttl)?.as_secs()`). A sub-second TTL
//! request (e.g. `"300ms"`) therefore truncates to `te.ttl == 0`, which — because our token's
//! policies never contain `"root"` — routes into `calculate_ttl`'s `backend_ttl > ZERO`
//! branch as FALSE, silently substituting `token_util::DEFAULT_LEASE_TTL` (24 HOURS) instead
//! of a near-instant expiry. **This is exactly why expiry in this module is decided from OUR
//! OWN [`MintedProfileToken::expires_at`] bookkeeping, computed in Rust from the `ttl`
//! parameter BEFORE it is ever formatted into a vault request, and never from what RustyVault
//! itself does with that value internally.** [`expired_token_read_surfaces_a_named_expiry_error`]
//! (in the test file) mints with a genuinely sub-second TTL specifically to prove this: the
//! read is refused as expired even though the underlying vault-side lease is nowhere near its
//! (silently-substituted, 24-hour) expiry — proof the named error variant comes from our
//! bookkeeping, not a vault response.
//!
//! # Fourth corrected finding: RustyVault cannot distinguish "expired" from "unrecognized"
//!
//! `TokenStore::check_token` (`token_store.rs:370-378`) does `self.lookup(token)` and returns
//! `RvError::ErrPermissionDenied` when the lookup finds nothing. The background expiration
//! sweep (`ExpirationManager::start_check_expired_lease_entries`, a 200ms tick) revokes an
//! expired token's lease by calling `token_store.revoke_tree(client_token)`, which DELETES the
//! entry from storage — so a genuinely-expired-and-swept token and a bogus/never-issued token
//! produce the IDENTICAL `ErrPermissionDenied` → our [`crate::error::VaultError::Denied`].
//! [`crate::error::VaultError::TokenExpired`] is never constructed from a vault response in
//! this module for that reason; it is constructed from [`MintedProfileToken::is_expired`]
//! alone, BEFORE any vault request is even issued for an expired token.
//!
//! # Bootstrap-only credential model (D-15, user-ruled checkpoint decision)
//!
//! D-15's locked text says: "size the TTL to the maximum task duration or define a re-mint
//! path." Its premise — that `KanbanConfig::dispatch_stale_timeout_seconds` (default 14400s)
//! bounds task duration — is FALSE, verified against live source:
//! `extend_live_pid_claims` (`ironhermes-kanban/src/dispatcher.rs:530-580`) re-extends any
//! `running` task with a live PID by `DEFAULT_CLAIM_TTL_SECONDS` (900,
//! `ironhermes-kanban/src/cas.rs:24`) on every tick, indefinitely, and `enforce_max_runtime`
//! (`dispatcher.rs:707-730`) skips every task whose `max_runtime_seconds` is `None`. Task
//! duration is genuinely unbounded, so no fixed TTL "covers" it. Separately, the worker's
//! entire credential bootstrap is ONE `dotenvy::from_path` call
//! (`ironhermes-cli/src/main.rs:1804-1808`) with no lazy fetch or refetch seam — the token is
//! used exactly once, seconds after spawn. Given this, the user selected (at Task 1's blocking
//! checkpoint) the **bootstrap-only** credential model: a short TTL sized to the
//! spawn-to-bootstrap window, no re-mint path. This is a documented, user-approved departure
//! from D-15's literal wording — see `51-03-SUMMARY.md` for the full record. Consequently
//! [`profile_token_ttl_for_bootstrap`] exists and `profile_token_ttl_for_lease` does NOT.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rusty_vault::core::Core;
use rusty_vault::logical::{Auth, Request};
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;

use crate::error::VaultError;
use crate::profile_paths::{profile_policy_name, profile_secret_path};
use crate::profile_policy::ensure_profile_policy;
use crate::rusty_vault_store::{map_rv_error, read_request, run_request};

/// Structural audit sink for D-07's "record the token accessor in the trajectory ledger"
/// requirement — a REQUIRED parameter of [`mint_profile_token`], not an `Option` and not
/// defaultable, so an un-audited mint does not compile. One method, mirroring the
/// `TrajectoryWriterHandle` cycle-break pattern (`ironhermes-core::commands::context`,
/// implemented by `ironhermes_trajectory::handle::TrajectoryWriterHandleImpl`).
///
/// `ironhermes-vault` must not depend on `ironhermes-trajectory` (leaf-crate rule), and
/// `ironhermes-kanban` depends on neither `ironhermes-vault` nor `ironhermes-trajectory`
/// (confirmed by reading `crates/ironhermes-kanban/Cargo.toml` — only `ironhermes-core`,
/// `-tools`, `-artifacts`). So the dispatcher cannot implement this trait directly: Plan 06's
/// `ironhermes-core::profile_credentials` facade is the intended adapter, wiring core's own
/// `TrajectoryWriterHandle` onto this trait, with the gateway runner and CLI dispatch supplying
/// the concrete writer. A future call site reaching for `ironhermes-trajectory` directly from
/// `ironhermes-kanban` is reaching for a dependency that is not there.
pub trait ProfileTokenAudit: Send + Sync {
    /// Record a successful mint: the profile slug, the token's ACCESSOR (never the token
    /// itself), and the granted TTL. Called ONLY after the vault mint has actually succeeded —
    /// a failed mint leaves the sink untouched, so the ledger cannot imply a credential exists
    /// that does not.
    fn record_mint(&self, slug: &str, accessor: &str, ttl: Duration) -> anyhow::Result<()>;
}

/// The minted, short-lived, single-policy (plus RustyVault's own unavoidable `"default"`),
/// non-renewable token a worker carries. Carries the secret token itself only as a
/// [`SecretString`] — never a plain `String` — and every other field is safe to record.
pub struct MintedProfileToken {
    token: SecretString,
    accessor: String,
    slug: String,
    policy: String,
    ttl: Duration,
    expires_at: Instant,
}

impl MintedProfileToken {
    /// The secret token itself. Callers must not log, print, or `Debug`-format this value's
    /// contents — only pass it to [`crate::profile_store::ProfileSecretStore::read_profile_secret_with_token`]
    /// or an equivalent authorized call.
    pub fn token(&self) -> &SecretString {
        &self.token
    }

    /// The safe identifier — an opaque UUID generated by THIS crate, unrelated to and never
    /// derived from the token's own bytes (RustyVault has no server-side "accessor" mechanism
    /// registered at this pinned rev — `auth/token/lookup-accessor` etc. exist only as client
    /// SDK bindings in `api/auth_token.rs`, with no matching backend route in
    /// `TokenStore::new_backend`'s path table — so this crate mints its own). This is what
    /// every downstream record (the trajectory ledger, dispatch refusal reasons, worker
    /// bootstrap failure messages) uses instead of the token.
    pub fn accessor(&self) -> &str {
        &self.accessor
    }

    /// The profile slug this token is scoped to.
    pub fn slug(&self) -> &str {
        &self.slug
    }

    /// The granted policy name (`profile-{slug}`).
    pub fn policy(&self) -> &str {
        &self.policy
    }

    /// The TTL this token was minted with (our own request value, not RustyVault's internal,
    /// whole-second-truncated copy — see the module doc's "Third corrected finding").
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Whether this token is past its OWN recorded expiry, decided entirely from this crate's
    /// bookkeeping — never from a vault response. See the module doc for why RustyVault's own
    /// error taxonomy cannot make this distinction reliably at any TTL, let alone a sub-second
    /// one.
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

/// Hand-written, NEVER derived — a derived `Debug` would print `token`'s secret bytes
/// verbatim. Mirrors `rusty_vault_store.rs::KeyMaterial`'s "intentionally derives neither
/// `Debug` nor `Display`" precedent, but here a `Debug` impl exists so callers can still log
/// everything EXCEPT the secret.
impl std::fmt::Debug for MintedProfileToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintedProfileToken")
            .field("accessor", &self.accessor)
            .field("slug", &self.slug)
            .field("policy", &self.policy)
            .field("ttl", &self.ttl)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Bridge one `Core::handle_request` call whose response carries an `Auth` block (token
/// mint/renew) onto a blocking-pool thread, mirroring `rusty_vault_store.rs::run_request`'s
/// exact bridge shape (Phase 51 Plan 01's settled `spawn_blocking` + `Handle::block_on`
/// strategy) — NOT a second, drift-prone copy of that mapping: [`map_rv_error`] is reused
/// directly. The only difference from `run_request` is the extraction target (`resp.auth`
/// here, `resp.data` there), because minting needs the `Auth` block, not a KV response body.
async fn run_auth_request(
    core: Arc<Core>,
    build: impl FnOnce() -> Request + Send + 'static,
) -> Result<Option<Auth>, VaultError> {
    tokio::task::spawn_blocking(move || {
        let mut req = build();
        let resp = tokio::runtime::Handle::current()
            .block_on(core.handle_request(&mut req))
            .map_err(map_rv_error)?;
        Ok(resp.and_then(|r| r.auth))
    })
    .await
    .map_err(|e| VaultError::Backend(format!("blocking vault task panicked: {e}")))?
}

/// Mint a short-lived, single-policy (see module doc for the `"default"` caveat), non-renewable
/// token scoped to `slug`, authorized by `root_token`. The ONLY mint entry point in this crate
/// — the audit sink is a required, non-`Option` parameter, so an un-audited mint does not
/// compile.
///
/// Ensures `profile-{slug}`'s policy exists first (delegating to
/// [`crate::profile_policy::ensure_profile_policy`] — never re-implemented here), mints with
/// EXACTLY that one policy passed explicitly (never an empty policy list, which is what would
/// let `handle_create`'s own logic inherit the parent/root's policies instead), `renewable:
/// false`, and records the mint to `sink` only after the vault has actually issued the token.
/// If `sink.record_mint` itself fails, this returns `Err` — an un-audited-but-live credential
/// is treated as a mint failure, not a partial success; the orphaned vault-side token entry is
/// harmless and self-expires via its own TTL.
pub async fn mint_profile_token(
    core: &Arc<Core>,
    root_token: &SecretString,
    slug: &str,
    ttl: Duration,
    sink: &dyn ProfileTokenAudit,
) -> Result<MintedProfileToken, VaultError> {
    let root = root_token.expose_secret().to_string();

    ensure_profile_policy(core, &root, slug).await?;
    let policy_name = profile_policy_name(slug)?;

    // Millisecond-precision request string (never a bare "{secs}s") — see the module doc's
    // "Third corrected finding": a request truncated to zero whole seconds silently becomes
    // RustyVault's 24-hour DEFAULT_LEASE_TTL instead of a short expiry. humantime's ms unit
    // keeps any TTL this crate is ever asked to mint (bootstrap-window seconds today) exact.
    let ttl_request = format!("{}ms", ttl.as_millis().max(1));
    let body = json!({
        "policies": [policy_name.clone()],
        "ttl": ttl_request,
        "renewable": false,
    });
    let body_map = body
        .as_object()
        .expect("json object literal is always a map")
        .clone();

    let core_for_req = Arc::clone(core);
    let root_for_req = root.clone();
    let auth = run_auth_request(core_for_req, move || {
        let mut req = Request::new_write_request("auth/token/create", Some(body_map));
        req.client_token = root_for_req;
        req
    })
    .await?;

    let auth = auth.ok_or_else(|| {
        VaultError::Backend("auth/token/create returned no auth block".to_string())
    })?;

    // Computed from OUR OWN `ttl` parameter, in Rust, before any vault request — never from
    // what RustyVault's request/response did with it internally. See the module doc.
    let expires_at = Instant::now() + ttl;
    let accessor = uuid::Uuid::new_v4().to_string();

    sink.record_mint(slug, &accessor, ttl)
        .map_err(|e| VaultError::Backend(format!("audit sink failed: {e}")))?;

    Ok(MintedProfileToken {
        token: SecretString::from(auth.client_token.clone()),
        accessor,
        slug: slug.to_string(),
        policy: policy_name,
        ttl,
        expires_at,
    })
}

/// Read a profile-scoped secret using a minted token, deciding "expired" from
/// [`MintedProfileToken::is_expired`] BEFORE issuing any vault request — see the module doc's
/// "Fourth corrected finding" for why RustyVault's own response cannot make this distinction.
/// Operates directly against `Arc<Core>` (the same bridge/decode shape
/// [`crate::profile_store::ProfileSecretStore::read_profile_secret_with_token`] uses) rather
/// than requiring a `ProfileSecretStore` — this crate exposes only one public constructor for
/// that type (`from_rusty_vault_store`, bound to a `RustyVaultStore`'s own internally-built
/// `Core`), so a caller who already holds a `Core` from this module's own `mint_profile_token`
/// has no way to wrap it in a `ProfileSecretStore` without opening a second vault. Plan 06/07
/// should call THIS function (not `ProfileSecretStore::read_profile_secret_with_token`
/// directly) so the expiry check always applies to a minted-token read.
pub async fn read_profile_secret_with_minted_token(
    core: &Arc<Core>,
    minted: &MintedProfileToken,
    leaf: &str,
) -> Result<Option<SecretString>, VaultError> {
    if minted.is_expired() {
        return Err(VaultError::TokenExpired);
    }
    let path = profile_secret_path(&minted.slug, leaf)?;
    let caller_token = minted.token.expose_secret().to_string();
    let core = Arc::clone(core);

    let data = run_request(core, move || read_request(&path, &caller_token)).await?;
    // Phase 51 Plan 17 (IN-03): reuses `profile_store::decode_value` directly rather than
    // keeping this file's own byte-identical copy — the earlier `files_modified` scoping
    // constraint that justified the duplicate belonged to the plan that imposed it, which is
    // done; see `decode_value`'s own doc.
    crate::profile_store::decode_value(data)
}

/// Stated spawn-to-bootstrap budget (D-15, bootstrap-only model): worst-case wall time from
/// `Command::spawn` to the worker's single UDS credential read (Plan 07), dominated by process
/// startup rather than the UDS round-trip itself (sub-millisecond on a healthy host). Includes
/// margin for a recorded, reproduced trap in this repo
/// (`env_macos_gatekeeper_nextest_dyld_stall`): a freshly-built binary can hang in
/// `_dyld_start` under macOS Gatekeeper's first-launch verification for tens of seconds — the
/// dominant real-world outlier this budget must absorb, not the steady-state case.
const SPAWN_TO_BOOTSTRAP_BUDGET_SECS: u64 = 20;

/// The bootstrap-only credential model's TTL (D-15, user-ruled at Task 1's checkpoint — see
/// the module doc). Three times [`SPAWN_TO_BOOTSTRAP_BUDGET_SECS`]: generous enough to absorb
/// the Gatekeeper stall above on a loaded dispatcher host, while staying two orders of
/// magnitude below `KanbanConfig::dispatch_stale_timeout_seconds`'s default (14400s) — the
/// token is used exactly once, so a longer TTL only widens T-51-13b's accepted
/// environment-block exposure with no benefit. Deliberately takes NO lease/timeout parameter:
/// the lease does not bound task duration (see module doc), so accepting one here would invite
/// a future call site to pass it and quietly restore a long TTL.
pub fn profile_token_ttl_for_bootstrap() -> Duration {
    Duration::from_secs(SPAWN_TO_BOOTSTRAP_BUDGET_SECS * 3)
}

//! Proves [`mint_profile_token`] against a real `rusty_vault` `Core` (Phase 51 D-07/D-08/D-15,
//! `51-03-PLAN.md`).
//!
//! Task 2: a token minted for `alpha` reads its own secret and is DENIED a sibling's and the
//! root `secret/providers/*` namespace — asserted on the denial, never on an absent `Ok`.
//! Policy removal kills an already-issued token immediately (the phase's only revocation
//! mechanism). The audit sink records the accessor and the TTL and provably never the token's
//! secret bytes; a failed mint records nothing. `MintedProfileToken`'s `Debug` redacts the
//! token.
//!
//! Task 3: exactly one TTL helper exists (bootstrap-only, per the user's Task 1 ruling); the
//! policy set and non-renewability are read back from a real `Core`, not asserted from our own
//! inputs; expiry is decided from our own bookkeeping and proven distinguishable from denial.
//!
//! # Corrected assertion: `minted_token_carries_exactly_one_policy_and_no_default`
//!
//! The name is unchanged from the plan (the `<verify>` nextest filter selects by name), but its
//! body is corrected against verified reality — see `profile_token.rs`'s module doc "Second
//! corrected finding". RustyVault's own `TokenStore::post_route` unconditionally re-attaches a
//! `"default"` policy to every token minted via `auth/token/create`, with no request-body field
//! able to suppress it at this pinned rev. This test therefore asserts the read-back set is
//! `{"profile-alpha", "default"}`, and the companion test `default_policy_grants_no_secret_access`
//! proves — independent of any code in this crate — that `"default"`'s own raw policy text
//! grants zero capability over `secret/profiles/*` or `secret/providers/*`. Together these
//! prove the SAME elevation-of-privilege property T-51-09 exists to close, honestly, rather
//! than asserting a literal single-element set that no implementation using the standard mint
//! API could ever produce.
#![cfg(feature = "rusty-vault")]

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ironhermes_vault::{
    ProfileTokenAudit, VaultError, ensure_profile_policy, mint_profile_token,
    profile_token_ttl_for_bootstrap, read_profile_secret_with_minted_token,
};
use rusty_vault::RustyVault;
use rusty_vault::core::{Core, SealConfig};
use rusty_vault::errors::RvError;
use rusty_vault::logical::Request;
use rusty_vault::modules::policy::acl::ACL;
use rusty_vault::modules::policy::policy::Policy;
use rusty_vault::storage;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

/// Construct a fresh, sealed `Arc<Core>` wired to a `file`-backed physical storage — mirrors
/// `tests/profile_policy_acl.rs::new_core` and `rusty_vault_store.rs::new_core` exactly (Plan
/// 01's settled construction sequence: bare `Core::new(backend).wrap()` leaves
/// `module_manager` empty and the first `init()` fails).
fn new_core(data_dir: &Path) -> Arc<Core> {
    let mut conf: HashMap<String, Value> = HashMap::new();
    conf.insert(
        "path".to_string(),
        Value::String(data_dir.to_string_lossy().into_owned()),
    );
    let backend = storage::new_backend("file", &conf).expect("construct file physical backend");
    let rv = RustyVault::new(backend, None).expect("construct rusty_vault core");
    rv.core.load_full()
}

/// Write a value directly through the real `Core`, authorized as root — the same request shape
/// `ProfileSecretStore`/`RustyVaultStore` use internally, reimplemented here (not imported)
/// because this test builds its own standalone `Core` rather than going through
/// `RustyVaultStore::open` (whose `Core` is private and not obtainable from outside the
/// crate — `core_handle`/`root_token_secret` are `pub(crate)`).
async fn write_secret_raw(core: &Arc<Core>, root_token: &str, path: &str, value: &str) {
    let mut req = Request::new_write_request(
        path,
        Some(
            serde_json::json!({ "value": value })
                .as_object()
                .expect("json object literal is always a map")
                .clone(),
        ),
    );
    req.client_token = root_token.to_string();
    core.handle_request(&mut req)
        .await
        .expect("write secret directly through the real Core");
}

/// Read a value directly through the real `Core` with an arbitrary caller token and path,
/// returning the raw `RvError` on failure — used for the sibling/root-namespace denial
/// assertions, which must observe the actual vault error variant rather than this crate's
/// mapped [`VaultError`].
#[allow(clippy::result_large_err)] // RvError (rusty_vault dep) is >=272 bytes; this helper must surface the raw variant
async fn read_secret_raw(
    core: &Arc<Core>,
    token: &SecretString,
    path: &str,
) -> Result<Option<String>, RvError> {
    let mut req = Request::new_read_request(path);
    req.client_token = token.expose_secret().to_string();
    let resp = core.handle_request(&mut req).await?;
    Ok(resp
        .and_then(|r| r.data)
        .and_then(|d| d.get("value").and_then(Value::as_str).map(str::to_string)))
}

/// Delete a registered policy directly through the real `Core`, authorized as root — the phase's
/// only revocation mechanism (D-07).
async fn delete_policy_raw(core: &Arc<Core>, root_token: &str, name: &str) {
    let mut req = Request::new_delete_request(format!("sys/policy/{name}"), None);
    req.client_token = root_token.to_string();
    core.handle_request(&mut req)
        .await
        .expect("delete policy directly through the real Core");
}

/// Look up a token's PERSISTED entry as root (bypasses ACL entirely — root's policy set always
/// grants root-privileged access), returning the raw JSON `data` map `handle_lookup` builds.
/// This is how the policy-set/renewability assertions read back what RustyVault actually
/// stored, rather than asserting the flags this test itself passed in.
async fn lookup_token_raw(
    core: &Arc<Core>,
    root_token: &str,
    token: &SecretString,
) -> serde_json::Map<String, Value> {
    let raw_token = token.expose_secret().to_string();
    let mut req = Request::new_read_request(format!("auth/token/lookup/{raw_token}"));
    req.client_token = root_token.to_string();
    let resp = core
        .handle_request(&mut req)
        .await
        .expect("lookup the minted token as root");
    resp.and_then(|r| r.data)
        .expect("lookup-by-id must return the token entry's data")
}

/// Attempt to renew a token by ID, authorized as root (bypasses ACL on the ROUTE itself, which
/// is irrelevant noise for this assertion — `renew_token`'s `renewable: false` refusal does not
/// depend on who the caller is, only on the TARGET token's own recorded lease). Returns the
/// raw `RvError` on failure.
#[allow(clippy::result_large_err)] // RvError (rusty_vault dep) is >=272 bytes; this helper must surface the raw variant
async fn renew_token_raw(
    core: &Arc<Core>,
    root_token: &str,
    token: &SecretString,
) -> Result<Option<Value>, RvError> {
    let raw_token = token.expose_secret().to_string();
    let mut req = Request::new_write_request(
        format!("auth/token/renew/{raw_token}"),
        Some(
            serde_json::json!({ "increment": 0 })
                .as_object()
                .expect("json object literal is always a map")
                .clone(),
        ),
    );
    req.client_token = root_token.to_string();
    let resp = core.handle_request(&mut req).await?;
    Ok(resp.map(|_| Value::Null))
}

/// Full fixture: a real, initialized, unsealed `Core`; `alpha`/`beta` profile policies
/// registered (Plan 02's `ensure_profile_policy`); a profile-scoped secret written at each of
/// `alpha`'s and `beta`'s own paths; and a root-namespace secret at `secret/providers/anthropic`
/// (the 46.8 operator-key namespace a profile token must never reach).
async fn open_fixture() -> (tempfile::TempDir, Arc<Core>, String) {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let core = new_core(tmp.path());

    let seal_config = SealConfig {
        secret_shares: 1,
        secret_threshold: 1,
    };
    let init_result = core.init(&seal_config).await.expect("core.init()");
    let unsealed = core
        .unseal(&init_result.secret_shares[0])
        .await
        .expect("core.unseal()");
    assert!(unsealed, "single-share unseal must return Ok(true)");
    let root_token = init_result.root_token.clone();

    ensure_profile_policy(&core, &root_token, "alpha")
        .await
        .expect("register alpha's policy");
    ensure_profile_policy(&core, &root_token, "beta")
        .await
        .expect("register beta's policy");

    write_secret_raw(
        &core,
        &root_token,
        "secret/profiles/alpha/openrouter",
        "sk-test-alpha",
    )
    .await;
    write_secret_raw(
        &core,
        &root_token,
        "secret/profiles/beta/openrouter",
        "sk-test-beta",
    )
    .await;
    write_secret_raw(
        &core,
        &root_token,
        "secret/providers/anthropic",
        "sk-root-anthropic",
    )
    .await;

    (tmp, core, root_token)
}

#[derive(Default)]
struct RecordingAudit {
    records: Mutex<Vec<(String, String, Duration)>>,
}

impl ProfileTokenAudit for RecordingAudit {
    fn record_mint(&self, slug: &str, accessor: &str, ttl: Duration) -> anyhow::Result<()> {
        self.records
            .lock()
            .expect("recording audit mutex must not be poisoned")
            .push((slug.to_string(), accessor.to_string(), ttl));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Task 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn minted_token_reads_own_secret_and_is_denied_siblings() {
    let (_tmp, core, root_token) = open_fixture().await;
    let sink = RecordingAudit::default();

    let minted = mint_profile_token(
        &core,
        &SecretString::from(root_token.clone()),
        "alpha",
        Duration::from_secs(30),
        &sink,
    )
    .await
    .expect("mint a token for alpha");

    // Own read succeeds.
    let own = read_profile_secret_with_minted_token(&core, &minted, "openrouter")
        .await
        .expect("own-profile read must succeed")
        .expect("secret must be present");
    assert_eq!(own.expose_secret(), "sk-test-alpha");

    // Sibling read is DENIED — asserted on the error variant, never on an absent Ok.
    let sibling = read_secret_raw(&core, minted.token(), "secret/profiles/beta/openrouter").await;
    assert!(
        matches!(sibling, Err(RvError::ErrPermissionDenied)),
        "alpha's token must be DENIED reading beta's subtree, got {sibling:?}"
    );

    // Root-namespace read is DENIED — the profile token must not reach the operator's own keys.
    let root_ns = read_secret_raw(&core, minted.token(), "secret/providers/anthropic").await;
    assert!(
        matches!(root_ns, Err(RvError::ErrPermissionDenied)),
        "alpha's token must be DENIED reading the root secret/providers/* namespace, got {root_ns:?}"
    );

    // Revocation: removing profile-alpha's policy kills the SAME already-issued token
    // immediately — the phase's only revocation mechanism (D-07).
    delete_policy_raw(&core, &root_token, "profile-alpha").await;
    let after_revoke = read_profile_secret_with_minted_token(&core, &minted, "openrouter").await;
    assert!(
        after_revoke.is_err(),
        "revoking profile-alpha's policy must kill the already-issued token, got {after_revoke:?}"
    );
}

#[tokio::test]
async fn mint_records_accessor_and_never_the_token_bytes() {
    let (_tmp, core, root_token) = open_fixture().await;
    let sink = RecordingAudit::default();

    let minted = mint_profile_token(
        &core,
        &SecretString::from(root_token),
        "alpha",
        Duration::from_secs(30),
        &sink,
    )
    .await
    .expect("mint a token for alpha");

    let records = sink
        .records
        .lock()
        .expect("recording audit mutex must not be poisoned");
    assert_eq!(records.len(), 1, "a successful mint must record exactly one entry");
    let (slug, accessor, ttl) = &records[0];
    assert_eq!(slug, "alpha");
    assert_eq!(accessor, minted.accessor());
    assert_eq!(*ttl, Duration::from_secs(30));

    let token_bytes = minted.token().expose_secret().to_string();
    assert_ne!(
        accessor.as_str(),
        token_bytes.as_str(),
        "the accessor must never equal the token itself"
    );
    assert!(
        !accessor.contains(token_bytes.as_str()) && !token_bytes.contains(accessor.as_str()),
        "the recorded accessor and the token must share no substring relationship"
    );
}

#[tokio::test]
async fn failed_mint_records_nothing() {
    let (_tmp, core, root_token) = open_fixture().await;
    let sink = RecordingAudit::default();

    // "UPPER" fails validate_profile_slug's positive allowlist (lowercase alphanumeric + '-'
    // only) — this fails inside ensure_profile_policy, well before the sink is ever touched.
    let result = mint_profile_token(
        &core,
        &SecretString::from(root_token),
        "UPPER",
        Duration::from_secs(30),
        &sink,
    )
    .await;
    assert!(result.is_err(), "an invalid slug must fail the mint");

    let records = sink
        .records
        .lock()
        .expect("recording audit mutex must not be poisoned");
    assert!(
        records.is_empty(),
        "a failed mint must leave the audit sink untouched, got {records:?}"
    );
}

#[tokio::test]
async fn minted_token_debug_redacts_the_secret() {
    let (_tmp, core, root_token) = open_fixture().await;
    let sink = RecordingAudit::default();

    let minted = mint_profile_token(
        &core,
        &SecretString::from(root_token),
        "alpha",
        Duration::from_secs(30),
        &sink,
    )
    .await
    .expect("mint a token for alpha");

    let token_bytes = minted.token().expose_secret().to_string();
    let debug_str = format!("{minted:?}");
    assert!(
        !debug_str.contains(&token_bytes),
        "Debug output must not contain the token's secret bytes: {debug_str}"
    );
    assert!(
        debug_str.contains(minted.accessor()),
        "Debug output should still surface the safe accessor: {debug_str}"
    );
}

// ---------------------------------------------------------------------------
// Task 3
// ---------------------------------------------------------------------------

#[test]
fn ttl_matches_the_ruled_credential_model() {
    // Bootstrap-only ruling (Task 1's user decision): strictly greater than the stated
    // spawn-to-bootstrap budget, strictly less than dispatch_stale_timeout_seconds's default
    // (14400s).
    let ttl = profile_token_ttl_for_bootstrap();
    assert!(
        ttl > Duration::from_secs(10),
        "TTL must exceed a real spawn-to-bootstrap budget, got {ttl:?}"
    );
    assert!(
        ttl < Duration::from_secs(14400),
        "TTL must stay well below dispatch_stale_timeout_seconds's default, got {ttl:?}"
    );
}

#[test]
fn only_the_ruled_ttl_helper_exists() {
    let source = include_str!("../src/profile_token.rs");
    let stripped: String = source
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let bootstrap_count = stripped.matches("fn profile_token_ttl_for_bootstrap").count();
    let lease_count = stripped.matches("fn profile_token_ttl_for_lease").count();
    assert_eq!(
        bootstrap_count, 1,
        "expected exactly one profile_token_ttl_for_bootstrap definition, got {bootstrap_count}"
    );
    assert_eq!(
        lease_count, 0,
        "profile_token_ttl_for_lease must not exist under the user-ruled bootstrap-only model"
    );
}

#[test]
fn module_doc_records_the_claim_extension_finding() {
    let source = include_str!("../src/profile_token.rs");
    assert!(
        source.contains("extend_live_pid_claims"),
        "module doc must name extend_live_pid_claims (the corrected D-15 premise)"
    );
    assert!(
        source.contains("max_runtime_seconds"),
        "module doc must name max_runtime_seconds (the corrected D-15 premise)"
    );
}

#[tokio::test]
async fn minted_token_carries_exactly_one_policy_and_no_default() {
    let (_tmp, core, root_token) = open_fixture().await;
    let sink = RecordingAudit::default();

    let minted = mint_profile_token(
        &core,
        &SecretString::from(root_token.clone()),
        "alpha",
        Duration::from_secs(30),
        &sink,
    )
    .await
    .expect("mint a token for alpha");

    let entry = lookup_token_raw(&core, &root_token, minted.token()).await;
    let policies: Vec<String> = entry
        .get("policies")
        .and_then(Value::as_array)
        .expect("lookup response must carry a policies array")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();

    let policy_set: std::collections::BTreeSet<String> = policies.into_iter().collect();
    let expected: std::collections::BTreeSet<String> = ["profile-alpha".to_string(), "default".to_string()]
        .into_iter()
        .collect();
    assert_eq!(
        policy_set, expected,
        "the persisted policy set must be exactly {{profile-alpha, default}} — see this file's \
         module doc for why RustyVault's own post_route unconditionally re-attaches \"default\" \
         at this pinned rev, with no request field able to suppress it"
    );
}

#[test]
fn default_policy_grants_no_secret_access() {
    // Pins the DEPENDENCY's own built-in "default" policy content directly — independent of
    // any code in this crate — proving the extra policy member
    // `minted_token_carries_exactly_one_policy_and_no_default` observes is genuinely inert for
    // the secret namespace, per T-51-09.
    const DEFAULT_POLICY: &str = rusty_vault::modules::policy::policy_store::DEFAULT_POLICY;
    let mut policy = Policy::from_str(DEFAULT_POLICY).expect("parse the real DEFAULT_POLICY HCL");
    policy.name = "default".to_string();
    let acl = ACL::new(&[Arc::new(policy)]).expect("build a real ACL from DEFAULT_POLICY");

    let alpha = Request::new_read_request("secret/profiles/alpha/openrouter");
    let alpha_result = acl
        .allow_operation(&alpha, false)
        .expect("evaluate a profile-scoped read under the default policy alone");
    assert!(
        !alpha_result.allowed,
        "the default policy must NOT grant secret/profiles/* access: {alpha_result:?}"
    );

    let root_ns = Request::new_read_request("secret/providers/anthropic");
    let root_ns_result = acl
        .allow_operation(&root_ns, false)
        .expect("evaluate a root-namespace read under the default policy alone");
    assert!(
        !root_ns_result.allowed,
        "the default policy must NOT grant secret/providers/* access: {root_ns_result:?}"
    );
}

#[tokio::test]
async fn minted_token_renewal_is_refused() {
    let (_tmp, core, root_token) = open_fixture().await;
    let sink = RecordingAudit::default();

    let minted = mint_profile_token(
        &core,
        &SecretString::from(root_token.clone()),
        "alpha",
        Duration::from_secs(30),
        &sink,
    )
    .await
    .expect("mint a token for alpha");

    let renewed = renew_token_raw(&core, &root_token, minted.token()).await;
    assert!(
        renewed.is_err(),
        "renewing a renewable:false token must fail — the actual renew attempt's result, not a \
         flag we passed in — got {renewed:?}"
    );
}

#[tokio::test]
async fn expired_token_read_surfaces_a_named_expiry_error() {
    let (_tmp, core, _root_token) = open_fixture().await;
    let sink = RecordingAudit::default();

    let minted = mint_profile_token(
        &core,
        &SecretString::from(_root_token.clone()),
        "alpha",
        Duration::from_millis(300),
        &sink,
    )
    .await
    .expect("mint a sub-second-TTL token for alpha");

    // A real elapsed sleep, not a mocked clock — the property under test is our own
    // Instant-based bookkeeping (see profile_token.rs's module doc: a sub-second TTL request
    // truncates to RustyVault's whole-second DEFAULT_LEASE_TTL internally, so the vault-side
    // lease is nowhere near expiry here — proof the named error comes from our bookkeeping,
    // not a vault response).
    tokio::time::sleep(Duration::from_millis(700)).await;

    let result = read_profile_secret_with_minted_token(&core, &minted, "openrouter").await;
    match result {
        Err(VaultError::TokenExpired) => {}
        other => panic!("expected Err(VaultError::TokenExpired), got {other:?}"),
    }
}

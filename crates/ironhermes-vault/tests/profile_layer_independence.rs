//! Standing evidence for D-08's "each layer is sufficient alone" claim and D-09's "prove the
//! layers independently, don't assert them" instruction (Phase 51, `51-04-PLAN.md`).
//!
//! The three named runs below (`native_acl_denies_cross_profile_without_the_guard`,
//! `guard_denies_cross_profile_when_the_acl_would_allow`,
//! `legitimate_read_completes_with_guard_registered`) are NOT three happy paths — each asserts
//! the OUTCOME of a cross-profile read AND the SIGNATURE of the layer that produced it (the
//! guard's own denial sink, empty or populated). Weakening any one of these three — for example,
//! relaxing Run A to only check the outcome and not the empty sink, or Run B to use the correct
//! D-05 policy instead of a deliberately permissive one — silently removes a security guarantee:
//! a completely inert guard and a completely inert ACL both produce a passing "the read was
//! denied" assertion as long as the OTHER layer works, which is exactly how Phase 47.4's GAP-7
//! shipped a gate that was correctly wired, fully green, and inert at its real call site.
//!
//! `auth_handlers` is `[policy_store, profile_guard]` in registration order (see
//! `profile_guard.rs`'s module doc, "Registration ordering facts") — `TokenStore`'s `post_auth`
//! loop (`src/modules/auth/token_store.rs:800-816`) returns on the FIRST non-`ErrHandlerDefault`
//! error, so with the correct D-05 policy in force `PolicyStore` denies first and
//! `ProfileGuard::post_auth` is never even invoked. This is what makes Run A's empty sink
//! STRUCTURAL rather than incidental: the guard cannot quietly be doing the work in that run,
//! because it is not reached.
//!
//! Task 3 adds a second, orthogonal proof: `pre_route_before_unseal_sees_no_policies` /
//! `pre_route_after_unseal_sees_policies` demonstrate — with a throwaway `Handler` test double,
//! never the production `ProfileGuard` — that the REJECTED `pre_route` placement's visibility
//! of `req.auth` depends entirely on unstated registration timing, and
//! `handler_registration_order_is_observed_not_assumed` pins the vector positions that
//! reasoning depends on as an observed fact.
#![cfg(feature = "rusty-vault")]

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use ironhermes_vault::{ProfileGuard, ProfileGuardAudit, register_profile_guard};
use rusty_vault::RustyVault;
use rusty_vault::core::{Core, SealConfig};
use rusty_vault::errors::RvError;
use rusty_vault::handler::{AuthHandler, Handler};
use rusty_vault::logical::{Auth, Operation, Request, Response};
use rusty_vault::storage;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Fixture — mirrors tests/profile_token_mint.rs's `new_core`/raw-request helpers exactly
// (this file builds its own standalone Core rather than going through RustyVaultStore, whose
// Core is private).
// ---------------------------------------------------------------------------

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

/// Init + 1-of-1 unseal a freshly-constructed `Core`, returning the root token. This is the
/// point at which `AuthModule::init`/`PolicyModule::init` run (`Core::post_unseal`), appending
/// `TokenStore` to `handlers` and `PolicyStore` to `auth_handlers` — see `handlers`/
/// `auth_handlers` assertions in `handler_registration_order_is_observed_not_assumed`.
async fn init_and_unseal(core: &Arc<Core>) -> String {
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
    init_result.root_token.clone()
}

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

/// Write a policy's raw HCL text directly through `sys/policy/<name>` — used both for the
/// correct D-05 shape (via `ensure_profile_policy`, imported below) and for the DELIBERATELY
/// PERMISSIVE policy Run B needs, which `ensure_profile_policy` can never produce (it always
/// renders the D-05 two-block shape).
async fn write_policy_raw(core: &Arc<Core>, root_token: &str, name: &str, hcl: &str) {
    let mut req = Request::new(format!("sys/policy/{name}"));
    req.operation = Operation::Write;
    req.client_token = root_token.to_string();
    req.body = Some(
        serde_json::json!({ "policy": hcl })
            .as_object()
            .expect("json object literal is always a map")
            .clone(),
    );
    core.handle_request(&mut req)
        .await
        .expect("write policy directly through the real Core");
}

/// Mint a token via a RAW `auth/token/create` request — never through
/// `ironhermes_vault::mint_profile_token`, which calls `ensure_profile_policy` internally and
/// would silently overwrite Run B's deliberately permissive policy text back to the safe D-05
/// shape before the read under test ever happens.
async fn create_token_raw(core: &Arc<Core>, root_token: &str, policies: &[&str]) -> SecretString {
    let mut req = Request::new_write_request(
        "auth/token/create",
        Some(
            serde_json::json!({
                "policies": policies,
                "ttl": "300s",
                "renewable": false,
            })
            .as_object()
            .expect("json object literal is always a map")
            .clone(),
        ),
    );
    req.client_token = root_token.to_string();
    let resp = core
        .handle_request(&mut req)
        .await
        .expect("mint a raw token directly through the real Core");
    let auth = resp
        .and_then(|r| r.auth)
        .expect("auth/token/create must return an auth block");
    SecretString::from(auth.client_token)
}

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

/// A `Core`, initialized and unsealed, with the CORRECT D-05 policies registered for `alpha`
/// and `beta`, each profile's own secret written, and a root-namespace secret written (for
/// `guard_passes_through_non_profile_paths`).
async fn open_correct_policy_fixture() -> (tempfile::TempDir, Arc<Core>, String) {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let core = new_core(tmp.path());
    let root_token = init_and_unseal(&core).await;

    ironhermes_vault::ensure_profile_policy(&core, &root_token, "alpha")
        .await
        .expect("register alpha's D-05 policy");
    ironhermes_vault::ensure_profile_policy(&core, &root_token, "beta")
        .await
        .expect("register beta's D-05 policy");

    write_secret_raw(&core, &root_token, "secret/profiles/alpha/openrouter", "sk-test-alpha").await;
    write_secret_raw(&core, &root_token, "secret/profiles/beta/openrouter", "sk-test-beta").await;
    write_secret_raw(&core, &root_token, "secret/providers/anthropic", "sk-root-anthropic").await;

    (tmp, core, root_token)
}

/// A `Core`, initialized and unsealed, with a DELIBERATELY PERMISSIVE policy registered under
/// the name `profile-alpha` — it grants the WHOLE `secret/profiles/*` subtree, which is what
/// the native ACL (layer 2) would allow if layer 3 (this guard) did not exist. This is Run B's
/// fixture (`guard_denies_cross_profile_when_the_acl_would_allow`) — the whole point is that
/// the policy NAME still matches the `profile-{slug}` convention this guard derives scope from,
/// while the policy's own granted content is broader than that slug's own subtree.
async fn open_permissive_policy_fixture() -> (tempfile::TempDir, Arc<Core>, String) {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let core = new_core(tmp.path());
    let root_token = init_and_unseal(&core).await;

    write_policy_raw(
        &core,
        &root_token,
        "profile-alpha",
        "path \"secret/profiles/*\" {\n  capabilities = [\"read\", \"list\"]\n}\n",
    )
    .await;

    write_secret_raw(&core, &root_token, "secret/profiles/alpha/openrouter", "sk-test-alpha").await;
    write_secret_raw(&core, &root_token, "secret/profiles/beta/openrouter", "sk-test-beta").await;

    (tmp, core, root_token)
}

#[derive(Default)]
struct RecordingGuardAudit {
    records: Mutex<Vec<(String, String, String)>>,
}

impl ProfileGuardAudit for RecordingGuardAudit {
    fn record_denial(&self, slug: &str, path: &str, policy_name: &str) -> anyhow::Result<()> {
        self.records
            .lock()
            .expect("recording audit mutex must not be poisoned")
            .push((slug.to_string(), path.to_string(), policy_name.to_string()));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Task 1 — the tracer + the guard's own standalone behavior
// ---------------------------------------------------------------------------

/// D-08's tracer: register the guard, use a policy the native ACL would ALLOW under (the
/// permissive fixture), and prove the guard alone denies a cross-profile read — the denial
/// sink records it. This is also Run B of Task 2's three-run independence proof.
#[tokio::test]
async fn guard_denies_cross_profile_when_the_acl_would_allow() {
    let (_tmp, core, root_token) = open_permissive_policy_fixture().await;
    let sink = Arc::new(RecordingGuardAudit::default());
    let _guard = register_profile_guard(&core, sink.clone()).expect("register the guard");

    let token = create_token_raw(&core, &root_token, &["profile-alpha"]).await;

    // The native ACL (layer 2) alone would ALLOW this — the policy text grants the whole
    // secret/profiles/* subtree. The guard (layer 3) must deny it anyway.
    let cross = read_secret_raw(&core, &token, "secret/profiles/beta/openrouter").await;
    assert!(
        matches!(cross, Err(RvError::ErrPermissionDenied)),
        "the guard must DENY a cross-profile read even under a policy the ACL would allow, got {cross:?}"
    );

    let records = sink.records.lock().expect("sink mutex must not be poisoned");
    assert_eq!(
        records.len(),
        1,
        "the guard's sink must record exactly one denial, got {records:?}"
    );
    let (slug, path, policy_name) = &records[0];
    assert_eq!(slug, "alpha");
    assert_eq!(path, "secret/profiles/beta/openrouter");
    assert_eq!(policy_name, "profile-alpha");
}

/// The hook is placed where the data exists: a legitimate own-profile read, driven end-to-end
/// through the real `Core::handle_request` pipeline with the guard registered, succeeds — which
/// is only possible if the guard observed a non-empty, correctly-scoped policy set on
/// `req.auth` at `post_auth` time and returned the chain's continue signal.
#[tokio::test]
async fn guard_sees_the_resolved_policy_set_at_post_auth() {
    let (_tmp, core, root_token) = open_correct_policy_fixture().await;
    let sink = Arc::new(RecordingGuardAudit::default());
    let _guard = register_profile_guard(&core, sink).expect("register the guard");

    let token = create_token_raw(&core, &root_token, &["profile-alpha"]).await;

    let own = read_secret_raw(&core, &token, "secret/profiles/alpha/openrouter")
        .await
        .expect("own-profile read must succeed with the guard registered");
    assert_eq!(own.as_deref(), Some("sk-test-alpha"));
}

/// Non-profile (root-namespace) traffic is untouched — the guard returns the continue signal
/// immediately for any path outside `secret/profiles/`, regardless of the presented token's
/// policies.
#[tokio::test]
async fn guard_passes_through_non_profile_paths() {
    let (_tmp, core, root_token) = open_correct_policy_fixture().await;
    let sink = Arc::new(RecordingGuardAudit::default());
    let _guard = register_profile_guard(&core, sink.clone()).expect("register the guard");

    let root_ns = read_secret_raw(
        &core,
        &SecretString::from(root_token.clone()),
        "secret/providers/anthropic",
    )
    .await
    .expect("root-namespace read with a root token must succeed while the guard is registered");
    assert_eq!(root_ns.as_deref(), Some("sk-root-anthropic"));

    assert!(
        sink.records.lock().expect("sink mutex").is_empty(),
        "a non-profile-path read must never reach the guard's denial sink"
    );
}

/// The guard's allowed prefix moves with the token's policy NAME, not with any parameter the
/// caller supplies. Two requests, identical except for the resolved policy name on `req.auth`,
/// produce different outcomes for the SAME path.
#[tokio::test]
async fn guard_derives_prefix_from_the_token_policy_name() {
    let sink = Arc::new(RecordingGuardAudit::default());
    let guard = ProfileGuard::new(sink.clone());

    let mut alpha_req = Request::new_read_request("secret/profiles/alpha/openrouter");
    alpha_req.auth = Some(Auth {
        policies: vec!["profile-alpha".to_string()],
        ..Auth::default()
    });
    let alpha_result = guard.post_auth(&mut alpha_req).await;
    assert_eq!(
        alpha_result,
        Err(RvError::ErrHandlerDefault),
        "alpha's own token reading alpha's own path must continue the chain, got {alpha_result:?}"
    );

    let mut beta_req = Request::new_read_request("secret/profiles/alpha/openrouter");
    beta_req.auth = Some(Auth {
        policies: vec!["profile-beta".to_string()],
        ..Auth::default()
    });
    let beta_result = guard.post_auth(&mut beta_req).await;
    assert_eq!(
        beta_result,
        Err(RvError::ErrPermissionDenied),
        "the SAME path under beta's token must be denied — the allowed prefix moved with the \
         policy name, got {beta_result:?}"
    );

    let records = sink.records.lock().expect("sink mutex");
    assert_eq!(records.len(), 1, "only the beta-token request should be denied");
    assert_eq!(records[0].0, "beta");
}

/// Every malformed policy-set shape denies, asserted per case: zero `profile-`-prefixed
/// policies, two-or-more, and one that resolves to a slug `profile_secret_prefix` itself
/// refuses (unparseable). Each case is a fresh request so a passing later case cannot mask a
/// failing earlier one.
#[tokio::test]
async fn guard_denies_on_malformed_policy_sets() {
    let sink = Arc::new(RecordingGuardAudit::default());
    let guard = ProfileGuard::new(sink);

    // Zero profile-prefixed policies — only unrelated policy names present.
    let mut zero_req = Request::new_read_request("secret/profiles/alpha/openrouter");
    zero_req.auth = Some(Auth {
        policies: vec!["default".to_string()],
        ..Auth::default()
    });
    let zero_result = guard.post_auth(&mut zero_req).await;
    assert_eq!(
        zero_result,
        Err(RvError::ErrPermissionDenied),
        "zero profile-prefixed policies must deny, got {zero_result:?}"
    );

    // Two-or-more profile-prefixed policies — ambiguous scope, must deny rather than pick one.
    let mut two_req = Request::new_read_request("secret/profiles/alpha/openrouter");
    two_req.auth = Some(Auth {
        policies: vec!["profile-alpha".to_string(), "profile-beta".to_string()],
        ..Auth::default()
    });
    let two_result = guard.post_auth(&mut two_req).await;
    assert_eq!(
        two_result,
        Err(RvError::ErrPermissionDenied),
        "two-or-more profile-prefixed policies must deny, got {two_result:?}"
    );

    // Unparseable slug — "UPPER" fails profile_secret_prefix's own validate_profile_slug rule
    // (must start with a lowercase letter or digit), so this exercises the guard's OWN
    // validation failure path, not a hand-rolled duplicate check.
    let mut bad_req = Request::new_read_request("secret/profiles/alpha/openrouter");
    bad_req.auth = Some(Auth {
        policies: vec!["profile-UPPER".to_string()],
        ..Auth::default()
    });
    let bad_result = guard.post_auth(&mut bad_req).await;
    assert_eq!(
        bad_result,
        Err(RvError::ErrPermissionDenied),
        "an unparseable derived slug must deny, got {bad_result:?}"
    );
}

/// Fail closed on an absent `req.auth` — unreachable through the normal `TokenStore::pre_route`
/// flow (which itself returns `ErrPermissionDenied` before ever invoking `post_auth` without a
/// resolved `Auth`), asserted directly here as defense in depth (D-08's own "every error path
/// denies" requirement).
#[tokio::test]
async fn guard_denies_when_req_auth_is_absent() {
    let sink = Arc::new(RecordingGuardAudit::default());
    let guard = ProfileGuard::new(sink);

    let mut req = Request::new_read_request("secret/profiles/alpha/openrouter");
    req.auth = None;

    let result = guard.post_auth(&mut req).await;
    assert_eq!(
        result,
        Err(RvError::ErrPermissionDenied),
        "a request reaching the guard with no resolved auth must be denied, got {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Task 2 — D-09's independence proof: Run A and Run C
// ---------------------------------------------------------------------------

/// Run A: correct D-05 policy, guard registered then explicitly REMOVED via
/// `delete_auth_handler` on the same Core the fixture just built — proving the toggle reaches
/// the live enforcement path (this is not a Core that simply never had the guard). The
/// cross-profile read is still denied (the native ACL alone), and the guard's OWN sink stays
/// empty — the load-bearing half: it is what distinguishes "the ACL denied this" from "the
/// guard we thought we removed denied this."
#[tokio::test]
async fn native_acl_denies_cross_profile_without_the_guard() {
    let (_tmp, core, root_token) = open_correct_policy_fixture().await;
    let sink = Arc::new(RecordingGuardAudit::default());
    let guard = register_profile_guard(&core, sink.clone()).expect("register the guard");
    // Remove it via the SAME Core's delete_auth_handler directly (not through a second
    // fixture) — this is what proves the toggle reaches the live TokenStore rather than being
    // cosmetic: Core::delete_auth_handler propagates to AuthModule::set_auth_handlers
    // (src/modules/auth/mod.rs:79), which is the exact mechanism `unregister_profile_guard`
    // wraps.
    core.delete_auth_handler(Arc::clone(&guard) as Arc<dyn AuthHandler>)
        .expect("remove the guard from the live Core via delete_auth_handler");

    let token = create_token_raw(&core, &root_token, &["profile-alpha"]).await;

    let cross = read_secret_raw(&core, &token, "secret/profiles/beta/openrouter").await;
    assert!(
        matches!(cross, Err(RvError::ErrPermissionDenied)),
        "the native ACL alone must still deny a cross-profile read, got {cross:?}"
    );

    assert!(
        sink.records.lock().expect("sink mutex").is_empty(),
        "the guard's sink must be EMPTY — it was removed before the read and must never have \
         been invoked"
    );
}

/// Run C: correct D-05 policy, guard registered (not removed), the profile reads its OWN key —
/// the chain must complete rather than the guard swallowing a legitimate request. Asserted on
/// the returned VALUE, not merely on the absence of an error.
#[tokio::test]
async fn legitimate_read_completes_with_guard_registered() {
    let (_tmp, core, root_token) = open_correct_policy_fixture().await;
    let sink = Arc::new(RecordingGuardAudit::default());
    let _guard = register_profile_guard(&core, sink.clone()).expect("register the guard");

    let token = create_token_raw(&core, &root_token, &["profile-alpha"]).await;

    let own = read_secret_raw(&core, &token, "secret/profiles/alpha/openrouter")
        .await
        .expect("own-key read must succeed with the guard registered");
    assert_eq!(own.as_deref(), Some("sk-test-alpha"));

    assert!(
        sink.records.lock().expect("sink mutex").is_empty(),
        "a legitimate own-key read must never reach the guard's denial sink"
    );
}

// ---------------------------------------------------------------------------
// Task 3 — the rejected pre_route placement's timing-dependent blindness, made observable
// ---------------------------------------------------------------------------

/// A throwaway `Handler` test double — NEVER the production `ProfileGuard`, which is an
/// `AuthHandler` and stays one. Records, for each `pre_route` invocation, whether `req.auth`
/// was `Some` at that moment. Always returns the chain's continue signal so it never actually
/// blocks a request — its only job is to observe.
struct AuthVisibilityProbe {
    handler_name: String,
    observed_auth_some: Mutex<Option<bool>>,
}

impl AuthVisibilityProbe {
    fn new(handler_name: &str) -> Arc<Self> {
        Arc::new(Self {
            handler_name: handler_name.to_string(),
            observed_auth_some: Mutex::new(None),
        })
    }
}

#[async_trait::async_trait]
impl Handler for AuthVisibilityProbe {
    fn name(&self) -> String {
        self.handler_name.clone()
    }

    async fn pre_route(&self, req: &mut Request) -> Result<Option<Response>, RvError> {
        *self
            .observed_auth_some
            .lock()
            .expect("probe mutex must not be poisoned") = Some(req.auth.is_some());
        Err(RvError::ErrHandlerDefault)
    }
}

/// A throwaway `Handler` test double registered via `add_handler` on a freshly-constructed,
/// NOT-yet-unsealed `Core` — i.e. BEFORE `AuthModule::init` appends `TokenStore` during unseal —
/// then driven by an authenticated request, observes `req.auth` as `None`. This is the original,
/// rejected `pre_route` design's failure mode made observable: registered this way, such a
/// handler's `pre_route` runs strictly BEFORE `TokenStore`'s own `pre_route` in the same
/// `handle_pre_route_phase` iteration, so it has no policy data to enforce against yet.
#[tokio::test]
async fn pre_route_before_unseal_sees_no_policies() {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let core = new_core(tmp.path());

    let probe = AuthVisibilityProbe::new("probe_before_unseal");
    core.add_handler(probe.clone() as Arc<dyn Handler>)
        .expect("register the probe BEFORE unseal");

    let root_token = init_and_unseal(&core).await;
    ironhermes_vault::ensure_profile_policy(&core, &root_token, "alpha")
        .await
        .expect("register alpha's policy");
    write_secret_raw(&core, &root_token, "secret/profiles/alpha/openrouter", "sk-test-alpha").await;
    let token = create_token_raw(&core, &root_token, &["profile-alpha"]).await;

    let mut req = Request::new_read_request("secret/profiles/alpha/openrouter");
    req.client_token = token.expose_secret().to_string();
    let _ = core.handle_request(&mut req).await;

    assert_eq!(
        *probe.observed_auth_some.lock().expect("probe mutex"),
        Some(false),
        "a Handler registered BEFORE unseal must observe req.auth as None at pre_route time"
    );
}

/// The SAME throwaway handler, registered AFTER unseal instead, observes `req.auth` as `Some` —
/// proving the difference is registration TIMING, not the `pre_route` hook being universally
/// blind to policy data.
#[tokio::test]
async fn pre_route_after_unseal_sees_policies() {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let core = new_core(tmp.path());
    let root_token = init_and_unseal(&core).await;

    let probe = AuthVisibilityProbe::new("probe_after_unseal");
    core.add_handler(probe.clone() as Arc<dyn Handler>)
        .expect("register the probe AFTER unseal");

    ironhermes_vault::ensure_profile_policy(&core, &root_token, "alpha")
        .await
        .expect("register alpha's policy");
    write_secret_raw(&core, &root_token, "secret/profiles/alpha/openrouter", "sk-test-alpha").await;
    let token = create_token_raw(&core, &root_token, &["profile-alpha"]).await;

    let mut req = Request::new_read_request("secret/profiles/alpha/openrouter");
    req.client_token = token.expose_secret().to_string();
    let _ = core.handle_request(&mut req).await;

    assert_eq!(
        *probe.observed_auth_some.lock().expect("probe mutex"),
        Some(true),
        "a Handler registered AFTER unseal must observe req.auth as Some at pre_route time"
    );
}

/// Pins the observed handler-vector positions directly, rather than inferring them from
/// source: `handlers` is `[router, token_store]` after unseal (before any of our own
/// registration), the router never gains a `pre_route`, and `auth_handlers` is
/// `[policy_store, profile_guard]` once this crate's guard registers.
#[tokio::test]
async fn handler_registration_order_is_observed_not_assumed() {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let core = new_core(tmp.path());
    let _root_token = init_and_unseal(&core).await;

    {
        let handlers = core.handlers.load();
        assert_eq!(handlers.len(), 2, "expected [router, token_store] after unseal");
        assert_eq!(handlers[0].name(), "core_router");
        assert_eq!(handlers[1].name(), "auth_token");
    }

    {
        let auth_handlers = core.auth_handlers.load();
        assert_eq!(
            auth_handlers.len(),
            1,
            "expected [policy_store] after unseal, before our own guard registers"
        );
        assert_eq!(auth_handlers[0].name(), "policy_store");
    }

    let sink = Arc::new(RecordingGuardAudit::default());
    let guard = register_profile_guard(&core, sink).expect("register the guard");

    {
        let auth_handlers = core.auth_handlers.load();
        assert_eq!(auth_handlers.len(), 2);
        assert_eq!(auth_handlers[0].name(), "policy_store");
        assert_eq!(auth_handlers[1].name(), guard.name());
    }

    let probe = AuthVisibilityProbe::new("probe_order_check");
    core.add_handler(probe as Arc<dyn Handler>)
        .expect("register a throwaway handler after unseal");

    {
        let handlers = core.handlers.load();
        assert_eq!(handlers.len(), 3, "a handler registered after unseal must land last");
        assert_eq!(handlers[2].name(), "probe_order_check");
    }
}


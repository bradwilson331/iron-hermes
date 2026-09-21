//! Pins per-profile ACL enforcement against the pinned `rusty_vault` rev
//! (`0922e6d5e6fbe6fd2d909863eca6d583623f4ad7`, v0.3.1) — Phase 51 D-05/D-07/D-09.
//!
//! `acl_allows_own_subtree_denies_sibling` proves D-05's rendered two-block policy, evaluated
//! through the pinned rev's own `ACL::new()`/`allow_operation`, grants a profile's own subtree
//! and refuses a sibling profile's identically-shaped subtree — built from the real rendered
//! HCL text, not a mock or a hand-written policy of our own. `ensure_profile_policy_is_idempotent`
//! proves the registrar (which needs a real, initialized, unsealed `Core`) leaves exactly one
//! registered policy after two calls for the same slug.
//!
//! Task 3 extends this file with three more tests that pin the ACL engine's overlap semantics
//! directly (not through our own policy) — see that section's module doc addendum below.
//!
//! # These tests pin the DEPENDENCY's semantics, not just our policy's correctness (D-09)
//!
//! `acl_allows_own_subtree_denies_sibling` and `ensure_profile_policy_is_idempotent` prove OUR
//! rendered policy behaves correctly. The three tests Task 3 adds below go further: they prove
//! the pinned rev's ACL engine resolves specificity, deny-precedence, and `+` segment wildcards
//! the way RESEARCH.md's read of `acl.rs` describes — pinned at rev
//! `0922e6d5e6fbe6fd2d909863eca6d583623f4ad7`, so a future dependency bump that silently changes
//! any of the three resolution mechanisms fails here loudly rather than widening access quietly.
#![cfg(feature = "rusty-vault")]

use std::{collections::HashMap, path::Path, str::FromStr, sync::Arc};

use ironhermes_vault::{ensure_profile_policy, render_profile_policy};
use rusty_vault::{
    RustyVault,
    core::{Core, SealConfig},
    logical::Request,
    modules::policy::{PolicyType, acl::ACL, policy::Policy},
    storage,
};
use serde_json::Value;

/// Construct a fresh, sealed `Arc<Core>` wired to a `file`-backed physical storage rooted at
/// `data_dir`. Kept as a local reimplementation (mirrors `rusty_vault_store.rs::new_core` and
/// `tests/rusty_vault_spike.rs::new_core` — see either file's doc for why: bare
/// `Core::new(backend).wrap()` leaves `module_manager` empty and the first `init()` fails).
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

#[test]
fn acl_allows_own_subtree_denies_sibling() {
    let policy_text = render_profile_policy("alpha").expect("render D-05 policy for alpha");
    let mut policy = Policy::from_str(&policy_text).expect("parse rendered HCL as an ACL policy");
    policy.name = "profile-alpha".to_string();

    let acl = ACL::new(&[Arc::new(policy)]).expect("build ACL from the real rendered policy");

    let own_request = Request::new_read_request("secret/profiles/alpha/openrouter");
    let own_result = acl
        .allow_operation(&own_request, false)
        .expect("evaluate a read of alpha's own subtree");
    assert!(
        own_result.allowed,
        "profile alpha must be able to read its own subtree: {own_result:?}"
    );

    let sibling_request = Request::new_read_request("secret/profiles/beta/openrouter");
    let sibling_result = acl
        .allow_operation(&sibling_request, false)
        .expect("evaluate a read of beta's subtree from alpha's policy");
    assert!(
        !sibling_result.allowed,
        "profile alpha must NOT be able to read profile beta's subtree: {sibling_result:?}"
    );
}

#[tokio::test]
async fn ensure_profile_policy_is_idempotent() {
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
    assert!(
        unsealed,
        "single-share unseal must return Ok(true) on the first call"
    );
    let root_token = init_result.root_token.clone();

    ensure_profile_policy(&core, &root_token, "alpha")
        .await
        .expect("first ensure_profile_policy call must succeed");
    ensure_profile_policy(&core, &root_token, "alpha")
        .await
        .expect("second ensure_profile_policy call must also succeed (idempotent)");

    let policy_module = core
        .module_manager
        .get_module::<rusty_vault::modules::policy::PolicyModule>("policy")
        .expect("PolicyModule must be registered by rusty_vault::RustyVault::new");
    let registered = policy_module
        .policy_store
        .load()
        .list_policy(PolicyType::Acl)
        .await
        .expect("list registered ACL policies");
    let count = registered.iter().filter(|name| *name == "profile-alpha").count();
    assert_eq!(
        count, 1,
        "exactly one profile-alpha policy must be registered after two ensure_profile_policy \
         calls, got: {registered:?}"
    );
}

// --- Task 3: pin the ACL's three distinct overlap mechanisms (D-09) ---
//
// These three tests deliberately build their own HCL text (D-05-shaped, but not via
// `render_profile_policy`) rather than reusing this crate's renderer, because their job is to
// pin the DEPENDENCY's resolution behavior, not to re-prove our own policy is correct (that is
// what `acl_allows_own_subtree_denies_sibling` above already does). Each test exercises a
// DIFFERENT code path inside `acl.rs` at the pinned rev — conflating them into one test would
// leave two of the three mechanisms unpinned while appearing to cover all three.

/// The trie `get_ancestor` fast path (D-05's actual shape: two plain trailing-`*` prefix
/// rules, no `+`). A request inside the longer, more specific stored prefix resolves to that
/// prefix's permissions; a request outside it but under the broader glob resolves to the
/// broader rule. Zero custom precedence code is involved — this is the radix trie's own
/// longest-matching-prefix behavior, pinned here rather than assumed.
#[test]
fn specific_allow_beats_broader_deny_by_path_specificity() {
    let hcl = r#"
path "secret/profiles/alpha/*" {
  capabilities = ["read", "list"]
}

path "secret/profiles/*" {
  capabilities = ["deny"]
}
"#;
    let mut policy = Policy::from_str(hcl).expect("parse D-05-shaped policy");
    policy.name = "acl-pin-specificity".to_string();
    let acl = ACL::new(&[Arc::new(policy)]).expect("build ACL from raw D-05-shaped HCL");

    let inside = Request::new_read_request("secret/profiles/alpha/openrouter");
    let inside_result = acl
        .allow_operation(&inside, false)
        .expect("evaluate a request inside the specific prefix");
    assert!(
        inside_result.allowed,
        "the longer, more specific stored prefix must win inside its own subtree: \
         {inside_result:?}"
    );

    let outside = Request::new_read_request("secret/profiles/beta/openrouter");
    let outside_result = acl
        .allow_operation(&outside, false)
        .expect("evaluate a request outside the specific prefix");
    assert!(
        !outside_result.allowed,
        "the broader deny must win outside the specific subtree: {outside_result:?}"
    );
}

/// The `ACL::new()` merge path (NOT D-05's actual shape, but D-09 requires it pinned anyway):
/// two policy blocks declaring the exact SAME literal path with conflicting capabilities.
/// `Permissions::merge` makes deny sticky in both directions — run BOTH declaration orders, so
/// a merge rule that only holds for one ordering is exposed as luck, not precedence.
#[test]
fn identical_path_deny_is_sticky_across_policy_merge() {
    let allow_hcl = r#"
path "secret/profiles/gamma/key" {
  capabilities = ["read"]
}
"#;
    let deny_hcl = r#"
path "secret/profiles/gamma/key" {
  capabilities = ["deny"]
}
"#;
    let mut allow_policy = Policy::from_str(allow_hcl).expect("parse allow policy");
    allow_policy.name = "acl-pin-merge-allow".to_string();
    let mut deny_policy = Policy::from_str(deny_hcl).expect("parse deny policy");
    deny_policy.name = "acl-pin-merge-deny".to_string();

    let request = Request::new_read_request("secret/profiles/gamma/key");

    let acl_allow_then_deny = ACL::new(&[Arc::new(allow_policy.clone()), Arc::new(deny_policy.clone())])
        .expect("build ACL, allow declared before deny");
    let result_allow_then_deny = acl_allow_then_deny
        .allow_operation(&request, false)
        .expect("evaluate allow-then-deny ordering");
    assert!(
        !result_allow_then_deny.allowed,
        "deny must be sticky when declared AFTER allow: {result_allow_then_deny:?}"
    );

    let acl_deny_then_allow = ACL::new(&[Arc::new(deny_policy), Arc::new(allow_policy)])
        .expect("build ACL, deny declared before allow");
    let result_deny_then_allow = acl_deny_then_allow
        .allow_operation(&request, false)
        .expect("evaluate deny-then-allow ordering");
    assert!(
        !result_deny_then_allow.allowed,
        "deny must be sticky when declared BEFORE allow: {result_deny_then_allow:?}"
    );
}

/// `+` segment wildcards are real and distinct from trailing `*` at the pinned rev. D-05's
/// shipped policy uses no `+` at all — this is pinned per D-09's explicit instruction to answer
/// the question with a positive assertion rather than leave it untested, since a future policy
/// that reaches for `+` needs to know it lands in the segment-wildcard map (linear scan, custom
/// `Ord`), not the trie.
#[test]
fn segment_wildcard_plus_is_supported_distinctly_from_trailing_star() {
    let hcl = r#"
path "secret/profiles/+/data" {
  capabilities = ["read"]
}

path "secret/other/*" {
  capabilities = ["read"]
}
"#;
    let mut policy = Policy::from_str(hcl).expect("parse + and * policy");
    policy.name = "acl-pin-wildcards".to_string();
    let acl = ACL::new(&[Arc::new(policy)]).expect("build ACL from + and * policy");

    // '+' matches exactly one segment.
    let one_segment = Request::new_read_request("secret/profiles/alpha/data");
    let one_segment_result = acl
        .allow_operation(&one_segment, false)
        .expect("evaluate a one-segment '+' match");
    assert!(
        one_segment_result.allowed,
        "'+' must match exactly one path segment: {one_segment_result:?}"
    );

    // '+' must NOT span a separator — a deeper path must not match.
    let two_segments = Request::new_read_request("secret/profiles/alpha/beta/data");
    let two_segments_result = acl
        .allow_operation(&two_segments, false)
        .expect("evaluate a multi-segment path against '+'");
    assert!(
        !two_segments_result.allowed,
        "'+' must NOT span a path separator: {two_segments_result:?}"
    );

    // A trailing '*' matches a prefix, independently of the '+' rule above.
    let prefix_match = Request::new_read_request("secret/other/anything/here");
    let prefix_result = acl
        .allow_operation(&prefix_match, false)
        .expect("evaluate a trailing-'*' prefix match");
    assert!(
        prefix_result.allowed,
        "a trailing '*' must match a prefix, distinctly from '+': {prefix_result:?}"
    );
}

//! Proves [`ProfileSecretStore`] against a real `rusty_vault` `Core` (Phase 51 D-04/D-06/D-08,
//! `51-09-PLAN.md`).
//!
//! Task 1: a profile-scoped secret round-trips, and a profile-scoped LIST reaches the profile
//! prefix and excludes root-level names — the top blocker the cross-AI review found.
//! Task 2: the adapter has no back door (every method refuses a pre-joined compound path and a
//! traversal leaf), and the permissive-leaf compatibility decision is proven against a real
//! `ACL`, not just the validator.
//! Task 3: a token-authorized read that structurally cannot reach the root token, three
//! distinguishable failure outcomes (unreachable/denied/absent), and a source-level invariant
//! that the profile address is built in exactly one place.

#![cfg(feature = "rusty-vault")]

use std::str::FromStr;
use std::sync::Arc;

use ironhermes_vault::{
    ProfileSecretStore, RustyVaultConfig, RustyVaultStore, SecretStore, VaultError,
    profile_secret_path, profile_secret_prefix, render_profile_policy,
};
use rusty_vault::logical::Request;
use rusty_vault::modules::policy::{acl::ACL, policy::Policy};
use secrecy::{ExposeSecret, SecretString};

/// Build a fresh, initialized, unsealed `RustyVaultStore` + `ProfileSecretStore` pair in a
/// throwaway `TempDir`, through the crate's own public `init`/`open` path (keyfile mode,
/// auto-unseal) — the settled `rusty_vault::RustyVault::new(backend, None)` construction
/// sequence recorded in `51-01-SUMMARY.md`.
fn open_fresh_store() -> (tempfile::TempDir, RustyVaultStore, ProfileSecretStore) {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let config = RustyVaultConfig {
        data_dir: tmp.path().join("vault"),
        unseal_mode: "keyfile".to_string(),
    };
    RustyVaultStore::init(&config).expect("vault init");
    let store = RustyVaultStore::open(&config).expect("vault open (auto-unseal)");
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    (tmp, store, profile_store)
}

/// Same shape, but left SEALED (`unseal_mode: "passphrase"`, never unlocked) — for the
/// unreachable-vault tests (Task 3).
fn open_sealed_store() -> (tempfile::TempDir, RustyVaultStore, ProfileSecretStore) {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let config = RustyVaultConfig {
        data_dir: tmp.path().join("vault"),
        unseal_mode: "passphrase".to_string(),
    };
    RustyVaultStore::init(&config).expect("vault init");
    let store = RustyVaultStore::open(&config).expect("vault open (left sealed)");
    assert!(
        store.is_sealed().expect("read seal state"),
        "fixture must be left sealed for the unreachable-vault tests"
    );
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    (tmp, store, profile_store)
}

// --- Task 1 ---

#[tokio::test]
async fn profile_store_round_trips_a_profile_scoped_secret() {
    let (_tmp, _store, profile_store) = open_fresh_store();
    let plaintext = "sk-test-openrouter-alpha";

    profile_store
        .put_profile_secret(
            "alpha",
            "openrouter",
            SecretString::from(plaintext.to_string()),
        )
        .await
        .expect("write profile-scoped secret");

    let read = profile_store
        .get_profile_secret_as_root("alpha", "openrouter")
        .await
        .expect("read profile-scoped secret")
        .expect("secret must exist after write");

    assert_eq!(read.expose_secret(), plaintext);
}

#[tokio::test]
async fn list_profile_secret_names_reaches_the_profiles_prefix() {
    let (_tmp, store, profile_store) = open_fresh_store();

    profile_store
        .put_profile_secret(
            "alpha",
            "openrouter",
            SecretString::from("alpha-profile-secret".to_string()),
        )
        .await
        .expect("write profile-scoped secret");

    // Root-level write through the EXISTING SecretStore trait — proves the two namespaces are
    // addressed independently and the profile listing must not pick this name up.
    store
        .put_secret(
            "openrouter",
            SecretString::from("root-level-secret".to_string()),
        )
        .await
        .expect("write root-level secret via the existing SecretStore trait");

    let names = profile_store
        .list_profile_secret_names("alpha")
        .await
        .expect("list alpha's profile-scoped secret names");

    assert_eq!(
        names,
        vec!["openrouter".to_string()],
        "expected exactly alpha's profile-scoped leaf name, got: {names:?}"
    );
}

#[tokio::test]
async fn list_profile_secret_names_is_empty_for_an_unwritten_profile() {
    let (_tmp, _store, profile_store) = open_fresh_store();

    let names = profile_store
        .list_profile_secret_names("never-written")
        .await
        .expect("listing an unwritten profile must not be an error");

    assert!(
        names.is_empty(),
        "expected an empty Vec for an unwritten profile, got: {names:?}"
    );
}

#[tokio::test]
async fn list_profile_secret_names_returns_names_never_values() {
    let (_tmp, _store, profile_store) = open_fresh_store();
    let plaintext = "sk-super-secret-value-must-not-leak";

    profile_store
        .put_profile_secret("alpha", "openrouter", SecretString::from(plaintext.to_string()))
        .await
        .expect("write profile-scoped secret");

    let names = profile_store
        .list_profile_secret_names("alpha")
        .await
        .expect("list alpha's profile-scoped secret names");

    assert_eq!(names, vec!["openrouter".to_string()]);
    for name in &names {
        assert_ne!(
            name, plaintext,
            "a returned name must never equal the written secret's value"
        );
        assert!(
            !name.contains(plaintext),
            "a returned name must never contain the written secret's bytes, got: {name:?}"
        );
    }
}

#[tokio::test]
async fn root_provider_namespace_is_unchanged_by_the_profile_store() {
    let (_tmp, store, profile_store) = open_fresh_store();

    // Root-level CRUD through the pre-existing SecretStore trait, exactly as it behaved before
    // this plan.
    store
        .put_secret(
            "openrouter",
            SecretString::from("root-openrouter-secret".to_string()),
        )
        .await
        .expect("root put_secret");
    let root_read = store
        .get_secret("openrouter")
        .await
        .expect("root get_secret")
        .expect("root secret must exist after write");
    assert_eq!(root_read.expose_secret(), "root-openrouter-secret");

    // A profile-scoped write for a DIFFERENT leaf must not appear in a root-level, unfiltered
    // list — proves the separation in the other direction from
    // `list_profile_secret_names_reaches_the_profiles_prefix`.
    profile_store
        .put_profile_secret(
            "alpha",
            "venice",
            SecretString::from("alpha-venice-secret".to_string()),
        )
        .await
        .expect("write profile-scoped secret");

    let root_names = store
        .list_secrets(None)
        .await
        .expect("root list_secrets(None)");
    assert_eq!(
        root_names,
        vec!["openrouter".to_string()],
        "root-level list must contain only root-level names, got: {root_names:?}"
    );

    store.delete_secret("openrouter").await.expect("root delete_secret");
    assert!(
        store
            .get_secret("openrouter")
            .await
            .expect("root get_secret after delete")
            .is_none(),
        "root-level key must be gone after delete_secret"
    );
}

#[tokio::test]
async fn delete_profile_secret_removes_only_that_leaf() {
    let (_tmp, _store, profile_store) = open_fresh_store();

    profile_store
        .put_profile_secret("alpha", "openrouter", SecretString::from("alpha-or".to_string()))
        .await
        .expect("write alpha/openrouter");
    profile_store
        .put_profile_secret("alpha", "venice", SecretString::from("alpha-venice".to_string()))
        .await
        .expect("write alpha/venice");
    profile_store
        .put_profile_secret("beta", "openrouter", SecretString::from("beta-or".to_string()))
        .await
        .expect("write beta/openrouter");

    profile_store
        .delete_profile_secret("alpha", "openrouter")
        .await
        .expect("delete alpha/openrouter");

    assert!(
        profile_store
            .get_profile_secret_as_root("alpha", "openrouter")
            .await
            .expect("read after delete")
            .is_none(),
        "deleted leaf must read back as absent"
    );
    assert_eq!(
        profile_store
            .get_profile_secret_as_root("alpha", "venice")
            .await
            .expect("read alpha/venice")
            .expect("alpha/venice must survive the delete")
            .expose_secret(),
        "alpha-venice"
    );
    assert_eq!(
        profile_store
            .get_profile_secret_as_root("beta", "openrouter")
            .await
            .expect("read beta/openrouter")
            .expect("beta/openrouter must survive the delete")
            .expose_secret(),
        "beta-or"
    );
}

// --- Task 2 ---

/// Every method handed a pre-joined compound path as the SLUG must refuse it, and no vault
/// request may have been issued as a side effect — asserted by checking nothing landed under
/// a real, valid slug afterward, not only by the returned error.
#[tokio::test]
async fn store_methods_refuse_a_prejoined_compound_path() {
    let (_tmp, _store, profile_store) = open_fresh_store();
    let compound_slug = "alpha/openrouter";
    let dummy_token = SecretString::from("unused-token".to_string());

    assert!(
        profile_store
            .put_profile_secret(compound_slug, "leaf", SecretString::from("v".to_string()))
            .await
            .is_err(),
        "put_profile_secret must refuse a compound slug"
    );
    assert!(
        profile_store
            .get_profile_secret_as_root(compound_slug, "leaf")
            .await
            .is_err(),
        "get_profile_secret_as_root must refuse a compound slug"
    );
    assert!(
        profile_store
            .delete_profile_secret(compound_slug, "leaf")
            .await
            .is_err(),
        "delete_profile_secret must refuse a compound slug"
    );
    assert!(
        profile_store
            .list_profile_secret_names(compound_slug)
            .await
            .is_err(),
        "list_profile_secret_names must refuse a compound slug"
    );
    assert!(
        profile_store
            .read_profile_secret_with_token(&dummy_token, compound_slug, "leaf")
            .await
            .is_err(),
        "read_profile_secret_with_token must refuse a compound slug"
    );

    // No vault request could have been issued as a side effect of any of the above — a real,
    // valid slug that none of the refused calls could ever have addressed must still be empty.
    let names = profile_store
        .list_profile_secret_names("alpha")
        .await
        .expect("listing a real, untouched slug must not itself error");
    assert!(
        names.is_empty(),
        "no key must have been written by any refused call, got: {names:?}"
    );
}

/// Every method handed a traversal-shaped LEAF must refuse it, with the same no-side-effect
/// proof as the compound-slug test above.
#[tokio::test]
async fn store_methods_refuse_a_traversal_leaf() {
    let (_tmp, _store, profile_store) = open_fresh_store();
    let traversal_leaf = "../beta/openrouter";
    let dummy_token = SecretString::from("unused-token".to_string());

    assert!(
        profile_store
            .put_profile_secret("alpha", traversal_leaf, SecretString::from("v".to_string()))
            .await
            .is_err(),
        "put_profile_secret must refuse a traversal leaf"
    );
    assert!(
        profile_store
            .get_profile_secret_as_root("alpha", traversal_leaf)
            .await
            .is_err(),
        "get_profile_secret_as_root must refuse a traversal leaf"
    );
    assert!(
        profile_store
            .delete_profile_secret("alpha", traversal_leaf)
            .await
            .is_err(),
        "delete_profile_secret must refuse a traversal leaf"
    );
    assert!(
        profile_store
            .read_profile_secret_with_token(&dummy_token, "alpha", traversal_leaf)
            .await
            .is_err(),
        "read_profile_secret_with_token must refuse a traversal leaf"
    );
    // list_profile_secret_names takes only a slug — it has no leaf parameter to refuse, so the
    // no-side-effect assertion below (which re-lists the same valid slug) is this method's only
    // relevant check in this test.

    let names = profile_store
        .list_profile_secret_names("alpha")
        .await
        .expect("listing a real, untouched slug must not itself error");
    assert!(
        names.is_empty(),
        "no key must have been written by any refused call, got: {names:?}"
    );
}

/// The adapter's refusal for a bad slug/leaf must be the SAME error `profile_paths`' own
/// validators produce — proving the adapter routes through them rather than carrying a local
/// copy that could drift.
#[tokio::test]
async fn store_refusals_reuse_the_shared_validators() {
    let (_tmp, _store, profile_store) = open_fresh_store();

    let bad_slug = "UPPERCASE-SLUG";
    let direct_slug_err = profile_secret_prefix(bad_slug)
        .expect_err("direct validator call must refuse an uppercase slug")
        .to_string();
    let adapter_slug_err = profile_store
        .put_profile_secret(bad_slug, "openrouter", SecretString::from("v".to_string()))
        .await
        .expect_err("adapter must refuse the same uppercase slug")
        .to_string();
    assert_eq!(
        adapter_slug_err, direct_slug_err,
        "adapter's slug refusal must equal the shared validator's own error text"
    );

    let bad_leaf = "has/a/slash";
    let direct_leaf_err = profile_secret_path("alpha", bad_leaf)
        .expect_err("direct validator call must refuse a slash-bearing leaf")
        .to_string();
    let adapter_leaf_err = profile_store
        .put_profile_secret("alpha", bad_leaf, SecretString::from("v".to_string()))
        .await
        .expect_err("adapter must refuse the same slash-bearing leaf")
        .to_string();
    assert_eq!(
        adapter_leaf_err, direct_leaf_err,
        "adapter's leaf refusal must equal the shared validator's own error text"
    );
}

/// A mixed-case, underscore-bearing provider leaf must round-trip through the real store with
/// its case preserved (T-51-57b compatibility).
#[tokio::test]
async fn mixed_case_leaf_round_trips_through_the_store() {
    let (_tmp, _store, profile_store) = open_fresh_store();

    profile_store
        .put_profile_secret(
            "alpha",
            "My_Provider",
            SecretString::from("mixed-case-secret".to_string()),
        )
        .await
        .expect("write a mixed-case leaf");

    let read = profile_store
        .get_profile_secret_as_root("alpha", "My_Provider")
        .await
        .expect("read the mixed-case leaf")
        .expect("mixed-case leaf must exist after write");
    assert_eq!(read.expose_secret(), "mixed-case-secret");

    let names = profile_store
        .list_profile_secret_names("alpha")
        .await
        .expect("list alpha's profile-scoped secret names");
    assert!(
        names.contains(&"My_Provider".to_string()),
        "expected the mixed-case leaf name preserved in the listing, got: {names:?}"
    );
}

/// The permissive-leaf decision proven against a REAL `ACL` built from the REAL rendered
/// policy — not just the validator. Mirrors `profile_policy_acl.rs`'s
/// `acl_allows_own_subtree_denies_sibling` pattern exactly, substituting a mixed-case leaf.
#[tokio::test]
async fn mixed_case_leaf_is_granted_by_the_profile_policy_and_denied_to_a_sibling() {
    let policy_text = render_profile_policy("alpha").expect("render D-05 policy for alpha");
    let mut policy = Policy::from_str(&policy_text).expect("parse rendered HCL as an ACL policy");
    policy.name = "profile-alpha".to_string();

    let acl = ACL::new(&[Arc::new(policy)]).expect("build ACL from the real rendered policy");

    let own_request = Request::new_read_request("secret/profiles/alpha/My_Provider");
    let own_result = acl
        .allow_operation(&own_request, false)
        .expect("evaluate a read of alpha's own mixed-case leaf");
    assert!(
        own_result.allowed,
        "profile alpha must be able to read its own mixed-case leaf: {own_result:?}"
    );

    let sibling_request = Request::new_read_request("secret/profiles/beta/My_Provider");
    let sibling_result = acl
        .allow_operation(&sibling_request, false)
        .expect("evaluate a read of beta's mixed-case leaf from alpha's policy");
    assert!(
        !sibling_result.allowed,
        "profile alpha must NOT be able to read profile beta's mixed-case leaf: \
         {sibling_result:?}"
    );
}

// --- Task 3 ---

/// A bogus (unrecognized) token reading an EXISTING path must be a named denial, never
/// `Ok(None)` and never the value — proving `read_profile_secret_with_token` actually carries
/// the caller's token onto the request rather than silently falling back to root.
#[tokio::test]
async fn bogus_token_read_is_a_named_denial_not_empty_ok() {
    let (_tmp, _store, profile_store) = open_fresh_store();

    profile_store
        .put_profile_secret(
            "alpha",
            "openrouter",
            SecretString::from("alpha-openrouter-secret".to_string()),
        )
        .await
        .expect("write profile-scoped secret as root, so the path genuinely exists");

    let bogus_token = SecretString::from("this-token-was-never-minted-by-anything".to_string());
    let result = profile_store
        .read_profile_secret_with_token(&bogus_token, "alpha", "openrouter")
        .await;

    match result {
        Err(VaultError::Denied) => {}
        Err(other) => panic!("expected VaultError::Denied for a bogus token, got: {other:?}"),
        Ok(None) => panic!(
            "a bogus token must be a named denial, not Ok(None) — Ok(None) means \
             \"authorized, but absent\", which a bogus token is not"
        ),
        Ok(Some(_)) => panic!("a bogus token must never yield the secret's value"),
    }
}

/// Against a sealed `Core`, every profile-store method must return a named unreachable error
/// rather than an empty success.
#[tokio::test]
async fn sealed_vault_profile_read_is_a_named_error_not_empty_ok() {
    let (_tmp, _store, profile_store) = open_sealed_store();
    let token = SecretString::from("irrelevant-token".to_string());

    assert!(
        matches!(
            profile_store
                .put_profile_secret("alpha", "openrouter", SecretString::from("v".to_string()))
                .await,
            Err(VaultError::Sealed)
        ),
        "put_profile_secret against a sealed vault must be VaultError::Sealed"
    );
    assert!(
        matches!(
            profile_store
                .get_profile_secret_as_root("alpha", "openrouter")
                .await,
            Err(VaultError::Sealed)
        ),
        "get_profile_secret_as_root against a sealed vault must be VaultError::Sealed"
    );
    assert!(
        matches!(
            profile_store.delete_profile_secret("alpha", "openrouter").await,
            Err(VaultError::Sealed)
        ),
        "delete_profile_secret against a sealed vault must be VaultError::Sealed"
    );
    assert!(
        matches!(
            profile_store.list_profile_secret_names("alpha").await,
            Err(VaultError::Sealed)
        ),
        "list_profile_secret_names against a sealed vault must be VaultError::Sealed"
    );
    assert!(
        matches!(
            profile_store
                .read_profile_secret_with_token(&token, "alpha", "openrouter")
                .await,
            Err(VaultError::Sealed)
        ),
        "read_profile_secret_with_token against a sealed vault must be VaultError::Sealed"
    );
}

/// A well-formed request for a leaf that was never written is `Ok(None)` — the ONE case that
/// is legitimately empty — kept distinguishable from the sealed/denied tests above.
#[tokio::test]
async fn absent_leaf_is_ok_none_not_an_error() {
    let (_tmp, _store, profile_store) = open_fresh_store();

    assert!(
        profile_store
            .get_profile_secret_as_root("alpha", "never-written")
            .await
            .expect("a well-formed absent-leaf read must not be an error")
            .is_none(),
        "an absent leaf must read back as Ok(None), not an error"
    );
}

/// Source-level invariant: the profile address literal is constructed in exactly ONE file
/// under `crates/ironhermes-vault/src/`, and that file is the path builder. The needle is
/// derived at RUNTIME from `profile_secret_prefix` (never written as a contiguous string
/// literal in THIS file) so this test cannot self-invalidate by matching its own source line —
/// the exact self-counting trap this repo has already shipped once.
#[test]
fn profile_paths_are_built_in_exactly_one_place() {
    let needle = {
        let full = profile_secret_prefix("probe").expect("probe slug is valid");
        full.strip_suffix("probe/")
            .expect("prefix builder must end with the slug segment")
            .to_string()
    };

    let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut matching_files = Vec::new();
    let mut stack = vec![src_dir];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let content = std::fs::read_to_string(&path).expect("read source file");
            let non_comment: String = content
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            if non_comment.contains(&needle) {
                matching_files.push(path);
            }
        }
    }

    assert_eq!(
        matching_files.len(),
        1,
        "expected exactly one source file to construct the profile address literal, found: \
         {matching_files:?}"
    );
    assert_eq!(
        matching_files[0].file_name().and_then(|n| n.to_str()),
        Some("profile_paths.rs"),
        "the one file constructing the profile address literal must be profile_paths.rs, \
         found: {matching_files:?}"
    );
}

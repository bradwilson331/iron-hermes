//! Phase 51 Task 1 (D-14) — the vault branch: a second, still fail-closed path to
//! `Allow` for a profile whose `.env` has been scrubbed of its provider key.
//!
//! Absent this plan, Plan 08's `.env` migration turns every profile it touches into
//! a total dispatch outage — `vault_backed_profile_with_scrubbed_env_is_allowed` is
//! the tracer proving that does not happen.
//!
//! Gated on `rusty-vault` (mirrors `ironhermes-vault/tests/profile_store.rs`) — every
//! test here needs a real `RustyVaultStore`/`ProfileSecretStore`, both of which only
//! exist when that feature is compiled in.
#![cfg(feature = "rusty-vault")]

use std::path::Path;

use ironhermes_core::config::{Config, ProviderConfig};
use ironhermes_core::dispatch_gate::{
    DispatchDecision, ProfileSecretLister, evaluate_profile_dispatch_at,
    evaluate_vault_branch_for_test,
};
use ironhermes_vault::{ProfileSecretStore, RustyVaultConfig, RustyVaultStore, VaultError};
use secrecy::SecretString;

/// Write a profile directory at `root/<name>/` with the given `Config` and `.env`
/// contents (`None` = no `.env` file at all). Mirrors
/// `ironhermes-cli/tests/dispatch_profile_gate.rs`'s own helper.
fn write_profile(root: &Path, name: &str, config: &Config, env_contents: Option<&str>) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("mkdir profile dir");
    config
        .save_to(&dir.join("config.yaml"))
        .expect("save_to config.yaml");
    if let Some(contents) = env_contents {
        std::fs::write(dir.join(".env"), contents).expect("write .env");
    }
}

/// A fixture `Config` for provider `openrouter`, which (unlike a keyless local
/// endpoint) declares a key source and so must go through full resolution.
fn openrouter_config() -> Config {
    let mut config = Config::default();
    config.model.provider = "openrouter".to_string();
    config.providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            api_key_env: Some("OPENROUTER_API_KEY".to_string()),
            ..Default::default()
        },
    );
    config
}

/// Build a fresh, initialized, unsealed `RustyVaultStore` + `ProfileSecretStore` pair
/// in a throwaway `TempDir` — mirrors `ironhermes-vault/tests/profile_store.rs`'s own
/// helper (the settled `init` then `open` construction sequence).
fn open_fresh_vault() -> (tempfile::TempDir, RustyVaultConfig, ProfileSecretStore) {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let rv_config = RustyVaultConfig {
        data_dir: tmp.path().join("vault"),
        unseal_mode: "keyfile".to_string(),
    };
    RustyVaultStore::init(&rv_config).expect("vault init");
    let store = RustyVaultStore::open(&rv_config).expect("vault open (auto-unseal)");
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    (tmp, rv_config, profile_store)
}

/// Same shape, but left SEALED (`unseal_mode: "passphrase"`, never unlocked) — for
/// the unreachable-vault case.
fn open_sealed_vault() -> (tempfile::TempDir, RustyVaultConfig) {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let rv_config = RustyVaultConfig {
        data_dir: tmp.path().join("vault"),
        unseal_mode: "passphrase".to_string(),
    };
    RustyVaultStore::init(&rv_config).expect("vault init");
    let store = RustyVaultStore::open(&rv_config).expect("vault open (left sealed)");
    assert!(
        store.is_sealed().expect("read seal state"),
        "fixture must be left sealed for the unreachable-vault test"
    );
    (tmp, rv_config)
}

fn enable_vault(config: &mut Config, rv_config: &RustyVaultConfig) {
    config.vault.enabled = true;
    config.vault.backend = "rusty-vault".to_string();
    config.vault.rusty_vault = rv_config.clone();
}

// ---------------------------------------------------------------------------
// The tracer: a vault-backed profile with a scrubbed .env dispatches.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn vault_backed_profile_with_scrubbed_env_is_allowed() {
    let (_vault_tmp, rv_config, profile_store) = open_fresh_vault();
    profile_store
        .put_profile_secret(
            "vaultbacked",
            "openrouter",
            SecretString::from("sk-fixture-vault-9f21ab".to_string()),
        )
        .await
        .expect("put profile secret");

    let profiles_tmp = tempfile::tempdir().expect("tempdir");
    let mut config = openrouter_config();
    enable_vault(&mut config, &rv_config);
    write_profile(profiles_tmp.path(), "vaultbacked", &config, Some(""));

    let decision = evaluate_profile_dispatch_at(profiles_tmp.path(), "vaultbacked").await;
    assert_eq!(
        decision,
        DispatchDecision::AllowFromVault,
        "a vault-backed profile with a scrubbed .env must be AllowFromVault, got {decision:?}"
    );
}

// ---------------------------------------------------------------------------
// An un-migrated profile is judged byte-identically.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dotenv_backed_profile_is_unchanged() {
    let profiles_tmp = tempfile::tempdir().expect("tempdir");
    let config = openrouter_config();
    // Vault disabled (the default): DispatchDecision::default posture, unchanged.
    write_profile(
        profiles_tmp.path(),
        "dotenvbacked",
        &config,
        Some("OPENROUTER_API_KEY=sk-fixture-dotenv-7c3e91\n"),
    );

    let decision = evaluate_profile_dispatch_at(profiles_tmp.path(), "dotenvbacked").await;
    assert_eq!(
        decision,
        DispatchDecision::Allow,
        "a .env-backed profile with the vault disabled must be the plain Allow unit variant, got {decision:?}"
    );
}

#[tokio::test]
async fn dotenv_backed_profile_is_allow_not_allow_from_vault() {
    let (_vault_tmp, rv_config, profile_store) = open_fresh_vault();
    profile_store
        .put_profile_secret(
            "midmigration",
            "openrouter",
            SecretString::from("sk-fixture-vault-also-present".to_string()),
        )
        .await
        .expect("put profile secret");

    let profiles_tmp = tempfile::tempdir().expect("tempdir");
    let mut config = openrouter_config();
    enable_vault(&mut config, &rv_config);
    write_profile(
        profiles_tmp.path(),
        "midmigration",
        &config,
        Some("OPENROUTER_API_KEY=sk-fixture-dotenv-still-here\n"),
    );

    let decision = evaluate_profile_dispatch_at(profiles_tmp.path(), "midmigration").await;
    assert_eq!(
        decision,
        DispatchDecision::Allow,
        "the .env path must run first and win even when the vault also holds the secret \
         (a profile mid-migration must not silently switch to the vault path), got {decision:?}"
    );
}

// ---------------------------------------------------------------------------
// Three distinguishable refusal reasons.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn vault_disabled_refuses_exactly_as_today() {
    let profiles_tmp = tempfile::tempdir().expect("tempdir");
    let config = openrouter_config();
    // No vault config at all — the default, disabled posture.
    write_profile(profiles_tmp.path(), "novault", &config, Some(""));

    let decision = evaluate_profile_dispatch_at(profiles_tmp.path(), "novault").await;
    let DispatchDecision::Refuse { reason } = decision else {
        panic!("vault-disabled with no .env key must Refuse, got {decision:?}");
    };
    assert_eq!(
        reason,
        "profile \"novault\" is configured for provider \"openrouter\" but no key for that \
         provider resolves from its .env",
        "vault-disabled must return today's exact reason string, unchanged"
    );
}

#[tokio::test]
async fn unreachable_vault_refuses_with_a_distinct_reason() {
    let (_vault_tmp, rv_config) = open_sealed_vault();

    let profiles_tmp = tempfile::tempdir().expect("tempdir");
    let mut config = openrouter_config();
    enable_vault(&mut config, &rv_config);
    write_profile(profiles_tmp.path(), "sealedvault", &config, Some(""));

    let decision = evaluate_profile_dispatch_at(profiles_tmp.path(), "sealedvault").await;
    let DispatchDecision::Refuse { reason } = decision else {
        panic!("a sealed vault must Refuse, got {decision:?}");
    };
    assert!(
        reason.contains("unreachable"),
        "an unreachable (sealed) vault must name unreachability distinctly; got: {reason}"
    );
    assert_ne!(
        reason,
        "profile \"sealedvault\" is configured for provider \"openrouter\" but no key for \
         that provider resolves from its .env",
        "the unreachable-vault reason must be distinct from the vault-disabled reason"
    );
}

#[tokio::test]
async fn vault_without_the_profile_secret_refuses() {
    let (_vault_tmp, rv_config, profile_store) = open_fresh_vault();
    // Reachable vault, but no secret for THIS profile+provider — write one for a
    // different provider so the vault is proven non-empty, not merely untouched.
    profile_store
        .put_profile_secret(
            "unconfiguredvault",
            "anthropic",
            SecretString::from("sk-fixture-wrong-provider".to_string()),
        )
        .await
        .expect("put profile secret for a different provider");

    let profiles_tmp = tempfile::tempdir().expect("tempdir");
    let mut config = openrouter_config();
    enable_vault(&mut config, &rv_config);
    write_profile(profiles_tmp.path(), "unconfiguredvault", &config, Some(""));

    let decision = evaluate_profile_dispatch_at(profiles_tmp.path(), "unconfiguredvault").await;
    let DispatchDecision::Refuse { reason } = decision else {
        panic!("a reachable vault with no matching secret must Refuse, got {decision:?}");
    };
    assert!(
        reason.contains("unconfiguredvault") && reason.contains("openrouter"),
        "the reason must name the profile and the provider; got: {reason}"
    );
    assert!(
        !reason.contains("unreachable"),
        "a reachable vault with no matching secret must not be confused with an \
         unreachable vault; got: {reason}"
    );
}

// ---------------------------------------------------------------------------
// Phase 51 Plan 15 (T-51-87): a vault secret with nowhere to be installed
// refuses rather than AllowFromVault.
// ---------------------------------------------------------------------------

/// A `providers.<name>` entry with the DEPRECATED literal `api_key: Some("")` (empty) and no
/// `api_key_env`, for a provider name that is NOT one of the three built-ins — this is the one
/// shape that reaches the vault branch at all with no resolvable env-var name: step 6's keyless
/// carve-out only fires when NEITHER `api_key_env` NOR `api_key` is `Some(..)` at all (an empty
/// `Some("")` still counts as "declares a key source" for that check, so it is NOT treated as a
/// legitimately keyless local endpoint), yet the endpoint's resolved key is empty after
/// `.trim()` — so `RefuseNoKey` fires and the vault branch runs, while
/// [`ironhermes_core::provider_env::provider_api_key_env_name`] has no `api_key_env` to read and
/// no built-in fallback for this name, so it resolves to `None`. (A `custom_providers` entry
/// does NOT reach this branch: `Config::load_from` migrates it into `providers.<name>` with BOTH
/// fields `None`, which step 6's carve-out then correctly treats as a legitimately keyless
/// endpoint and allows directly — never touching the vault at all.)
#[tokio::test]
async fn vault_backed_provider_with_empty_literal_key_and_no_resolvable_env_var_refuses() {
    let (_vault_tmp, rv_config, profile_store) = open_fresh_vault();
    profile_store
        .put_profile_secret(
            "emptylegacykey",
            "myweirdprovider",
            SecretString::from("sk-fixture-empty-legacy-key".to_string()),
        )
        .await
        .expect("put profile secret");

    let profiles_tmp = tempfile::tempdir().expect("tempdir");
    let mut config = Config::default();
    config.model.provider = "myweirdprovider".to_string();
    config.providers.insert(
        "myweirdprovider".to_string(),
        ProviderConfig {
            base_url: Some("https://example.invalid".to_string()),
            api_key: Some(String::new()),
            ..Default::default()
        },
    );
    enable_vault(&mut config, &rv_config);
    write_profile(profiles_tmp.path(), "emptylegacykey", &config, Some(""));

    let decision = evaluate_profile_dispatch_at(profiles_tmp.path(), "emptylegacykey").await;
    let DispatchDecision::Refuse { reason } = decision else {
        panic!(
            "a vault-backed dispatch with no resolvable env-var name must Refuse rather than \
             AllowFromVault, got {decision:?}"
        );
    };
    assert!(
        reason.contains("emptylegacykey") && reason.contains("myweirdprovider"),
        "the refusal must name the profile and the provider; got: {reason}"
    );
    assert!(
        !reason.to_lowercase().contains("sk-fixture"),
        "the refusal must never echo any .env/vault content (T-51-25's control); got: {reason}"
    );
}

// ---------------------------------------------------------------------------
// Names-only listing, proven at runtime rather than by convention.
// ---------------------------------------------------------------------------

/// A double that returns canned names for the listing and inherits
/// [`ProfileSecretLister::get_profile_secret_as_root`]'s panicking default — any
/// value-returning call panics.
struct NamesOnlyDouble {
    names: Vec<String>,
}

#[async_trait::async_trait]
impl ProfileSecretLister for NamesOnlyDouble {
    async fn list_profile_secret_names(&self, _slug: &str) -> Result<Vec<String>, VaultError> {
        Ok(self.names.clone())
    }
}

#[tokio::test]
async fn vault_branch_uses_names_only_listing() {
    let double = NamesOnlyDouble {
        names: vec!["openrouter".to_string()],
    };

    // The branch itself only ever calls list_profile_secret_names — proven by the
    // fact this completes without panicking, and by the AllowFromVault result.
    // `openrouter` is one of the three built-in provider names, so
    // `Config::default()` (no `providers:` entry at all) still resolves an
    // env-var name for it — this test is about the LISTING seam, not the
    // env-var-name refusal (see `vault_backed_custom_provider_with_no_resolvable_env_var_refuses`
    // below for that).
    let decision =
        evaluate_vault_branch_for_test("nameonly", "openrouter", &Config::default(), &double).await;
    assert_eq!(
        decision,
        DispatchDecision::AllowFromVault,
        "a names-only listing containing the provider must be AllowFromVault, got {decision:?}"
    );

    // Prove the double is live: calling its value-returning method DOES panic, so
    // the property above is enforced at runtime, not merely by the double never
    // being exercised. Driven through `tokio::spawn` (rather than a nested
    // `block_on`, which would panic on its own — we are already inside a runtime)
    // so the panic surfaces as a `JoinError` instead of unwinding this test task.
    let panic_double = NamesOnlyDouble { names: Vec::new() };
    let join_result = tokio::spawn(async move {
        ProfileSecretLister::get_profile_secret_as_root(&panic_double, "nameonly", "openrouter")
            .await
    })
    .await;
    assert!(
        join_result.is_err_and(|e| e.is_panic()),
        "the double's get_profile_secret_as_root must panic when called — proves the \
         names-only property is a runtime guarantee, not an untested convention"
    );
}

// ---------------------------------------------------------------------------
// Task 3 — no production dispatch path kept the vault-blind entry point.
//
// Phase 47.4 Plan 10 put this exact predicate in `ironhermes-cli`, wired only
// into the one-shot `kanban dispatch` subcommand — the dispatcher that actually
// runs in production (`run_dispatch_loop`, spawned by the gateway) never went
// near it. The gate was correct, wired, and green — and inert. This plan
// reintroduces exactly the conditions for that failure by creating TWO entry
// points (`evaluate_profile_dispatch[_at]` and `evaluate_profile_dispatch_dotenv[_at]`)
// where there was one; this test is the invariant that closes it.
// ---------------------------------------------------------------------------

/// One production dispatch-path source file: `path` relative to the workspace
/// `crates/` directory, `min_vault_aware` the minimum required occurrences of
/// the vault-aware entry point's name (`evaluate_profile_dispatch`, which is a
/// prefix of both `evaluate_profile_dispatch` and `evaluate_profile_dispatch_at`),
/// and `production_only_boundary` an optional marker string: when present, only
/// the slice of the file BEFORE that marker is counted (excludes the file's own
/// `#[cfg(test)]` modules).
struct ProductionFile {
    path: &'static str,
    min_vault_aware: usize,
    production_only_boundary: Option<&'static str>,
}

const PRODUCTION_FILES: &[ProductionFile] = &[
    ProductionFile {
        path: "ironhermes-kanban/src/dispatcher.rs",
        min_vault_aware: 1,
        production_only_boundary: None,
    },
    ProductionFile {
        path: "ironhermes-cli/src/kanban/dispatch_gate.rs",
        min_vault_aware: 1,
        production_only_boundary: None,
    },
    ProductionFile {
        path: "iron_hermes_ui/src/server/profile_api.rs",
        min_vault_aware: 3,
        // profile_api.rs also references the gate inside its own `#[cfg(test)]`
        // modules (`profile_health_tests` is the first of six) — those are
        // legitimate and must not inflate or satisfy the production count.
        // Scoped by reading only the region BEFORE the first test module,
        // rather than excluding each `mod ..._tests { }` block individually —
        // sanctioned by this plan's own Task 3 action text ("scope the read to
        // the pre-`mod tests` region"). Every production call site in this file
        // (the `compute_provider_key_state` match, `create_profile_impl`'s and
        // `sync_profile_secrets_impl`'s post-write re-checks) sits before this
        // boundary; every test-only occurrence sits after it.
        production_only_boundary: Some("mod profile_health_tests"),
    },
    ProductionFile {
        path: "iron_hermes_ui/src/server/profile_verify_api.rs",
        min_vault_aware: 1,
        production_only_boundary: None,
    },
];

/// `crates/` — the parent of every production file above, resolved from this
/// test binary's own crate root (`crates/ironhermes-core`).
fn crates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("ironhermes-core's manifest dir must have a parent")
        .to_path_buf()
}

/// Strip full-line `//`/`///`/`//!` comments (the ONLY comment style these four
/// files use) before counting — doc comments legitimately discuss both entry
/// points, and a naive count would make this invariant self-invalidating the
/// moment someone documents it.
fn strip_comment_lines(src: &str) -> String {
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

const DOTENV_ONLY_NAME: &str = "evaluate_profile_dispatch_dotenv";
const VAULT_AWARE_NAME: &str = "evaluate_profile_dispatch";

/// Count occurrences of the vault-aware name, `VAULT_AWARE_NAME`, EXCLUDING any
/// that are actually part of the longer `.env`-only name — every
/// `DOTENV_ONLY_NAME` occurrence also contains `VAULT_AWARE_NAME` as a prefix
/// substring, so a naive `matches(VAULT_AWARE_NAME).count()` would double-count.
fn count_vault_aware_only(src: &str) -> usize {
    src.matches(VAULT_AWARE_NAME).count() - src.matches(DOTENV_ONLY_NAME).count()
}

#[test]
fn no_production_path_calls_the_dotenv_only_gate() {
    let root = crates_root();
    for file in PRODUCTION_FILES {
        let full_src = std::fs::read_to_string(root.join(file.path))
            .unwrap_or_else(|e| panic!("read {}: {e}", file.path));
        let scoped_src = match file.production_only_boundary {
            Some(marker) => {
                let idx = full_src.find(marker).unwrap_or_else(|| {
                    panic!(
                        "{}: expected boundary marker {marker:?} to exist in the file",
                        file.path
                    )
                });
                &full_src[..idx]
            }
            None => full_src.as_str(),
        };
        let stripped = strip_comment_lines(scoped_src);

        let dotenv_count = stripped.matches(DOTENV_ONLY_NAME).count();
        assert_eq!(
            dotenv_count, 0,
            "{}: the `.env`-only entry point's name ({DOTENV_ONLY_NAME:?}) must never \
             appear in a production dispatch path — found {dotenv_count} occurrence(s). \
             A surviving sync, vault-blind entry point reachable from production dispatch \
             reintroduces Phase 47.4's GAP-7.",
            file.path
        );

        let vault_aware_count = count_vault_aware_only(&stripped);
        assert!(
            vault_aware_count >= file.min_vault_aware,
            "{}: expected at least {} production occurrence(s) of the vault-aware entry \
             point's name ({VAULT_AWARE_NAME:?}), found {vault_aware_count} — a call site \
             may have silently reverted to the `.env`-only core.",
            file.path,
            file.min_vault_aware
        );
    }
}

// ---------------------------------------------------------------------------
// CR-05 / T-51-67 (Plan 17): the gate's vault branch must not deadlock when
// driven from a current-thread tokio runtime — the exact flavor
// `rusty_vault_feature_reaches_bot_spawn.rs` (`iron_hermes_ui`) records as
// hazardous for `RustyVaultStore::open`'s sync bridge
// (`block_on_admin_call`'s `rx.recv()` on the calling thread).
//
// A genuine hang here is NOT detectable by an in-runtime `tokio::time::timeout`:
// if `block_on_admin_call`'s `rx.recv()` blocks the runtime's only OS thread
// without ever yielding via `.await`, that same thread can never poll a
// timeout timer either — the whole point of a current-thread runtime is that
// there IS only one thread. The bound has to come from OUTSIDE that runtime:
// this test drives the target on its own fresh thread (with its own fresh
// current-thread runtime, mirroring `#[tokio::test]`'s default flavor
// faithfully) and waits on a plain `std::sync::mpsc::recv_timeout` from the
// test's own (unrelated) thread. A hang there reports as a clean test
// failure in bounded time instead of wedging the suite.
// ---------------------------------------------------------------------------

#[test]
fn gate_vault_branch_does_not_deadlock_under_current_thread_runtime() {
    // Setup (vault init/open + secret write) is deliberately done OUTSIDE the
    // watched section and its own throwaway runtime — this plan does not
    // touch profile-secret writes, and a hang here would be false RED
    // evidence for the wrong code path.
    let (_vault_tmp, rv_config, profile_store) = open_fresh_vault();
    let seed_rt = tokio::runtime::Runtime::new().expect("build seed runtime");
    seed_rt.block_on(async {
        profile_store
            .put_profile_secret(
                "gatecurrentthread",
                "openrouter",
                SecretString::from("sk-fixture-gate-current-thread-9f21ab".to_string()),
            )
            .await
            .expect("put profile secret")
    });
    drop(seed_rt);

    let profiles_tmp = tempfile::tempdir().expect("tempdir");
    let mut config = openrouter_config();
    enable_vault(&mut config, &rv_config);
    write_profile(profiles_tmp.path(), "gatecurrentthread", &config, Some(""));
    let profiles_root = profiles_tmp.path().to_path_buf();

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // The exact flavor `#[tokio::test]` defaults to — built manually here
        // so the watchdog above can live on a genuinely separate OS thread.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread runtime");
        let decision =
            rt.block_on(evaluate_profile_dispatch_at(&profiles_root, "gatecurrentthread"));
        let _ = tx.send(decision);
    });

    match rx.recv_timeout(std::time::Duration::from_secs(15)) {
        Ok(decision) => assert_eq!(
            decision,
            DispatchDecision::AllowFromVault,
            "the gate's vault branch, driven under a current-thread runtime, must resolve \
             to AllowFromVault, got {decision:?}"
        ),
        Err(_) => panic!(
            "the gate's vault branch deadlocked under a current-thread tokio runtime within \
             15s — this is precisely the `block_on_admin_call` hazard `open_async` (CR-05 / \
             T-51-67) exists to make unreachable from async code"
        ),
    }
}

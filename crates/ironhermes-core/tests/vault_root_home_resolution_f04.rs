//! Phase 51 UAT finding F-04 — regression test for the vault `data_dir`
//! sentinel resolving against a `--profile`-pivoted `IRONHERMES_HOME` instead
//! of the operator's ROOT home.
//!
//! # The bug this reproduces
//!
//! `resolve_vault_config`'s empty `rusty_vault.data_dir` sentinel used to fill
//! with `get_hermes_home()` — the CURRENT `IRONHERMES_HOME`. A kanban worker
//! runs with `IRONHERMES_HOME` pivoted to its own profile directory
//! (`resolve_and_set_profile`, `ironhermes-cli/src/main.rs`), so under a
//! worker the sentinel silently meant "a vault *inside this profile*" — an
//! address this system never creates. A worker with `vault.enabled: true` and
//! ANY keyless provider then died opening that nonexistent store with
//! `VaultError::NotInitialized` inside `ProviderResolver::apply_vault_fallback`
//! (`ironhermes-core/src/provider.rs:922`), AFTER it had already obtained its
//! own credential over the socket bootstrap.
//!
//! # What this test proves
//!
//! `profile_pivoted_worker_with_keyless_provider_does_not_die` reproduces the
//! exact failure shape end-to-end at this crate's layer: a real vault
//! initialized at the ROOT location, a `Config` with a keyless sibling
//! provider and `vault.enabled: true`, and `IRONHERMES_HOME`/
//! `IRONHERMES_ROOT_HOME` set exactly as `resolve_and_set_profile` leaves them
//! after a `--profile` pivot. `resolve_vault_config()` (the impure,
//! env-reading variant every production call site uses) must resolve to the
//! ROOT vault, `open_store` must succeed, and `apply_vault_fallback` must not
//! hard-error for the keyless provider.
//!
//! `without_root_home_stash_the_sentinel_resolves_to_the_pivoted_profile_and_fails`
//! is the negative control: the identical setup, but with the root-home stash
//! absent (reproducing every pre-fix call site, and any future regression
//! that stops stashing it), reproduces the ORIGINAL `NotInitialized` failure —
//! proving the positive test above actually exercises F-04's mechanism rather
//! than passing vacuously. Run together, these two tests are the RED
//! (negative control fails exactly as F-04 described) / GREEN (positive test
//! passes once the root-home stash is in effect) pair this defect fix
//! requires.
//!
//! `#![cfg(feature = "rusty-vault")]` — needs the concrete `RustyVaultStore`
//! type, matching this crate's other vault integration tests
//! (`vault_resolve_integration.rs`, `dispatch_gate_vault_backed.rs`).
//!
//! `env_lock()` mirrors the project-wide pattern (`provider.rs`'s own test
//! module) — required because this test mutates process-global
//! `IRONHERMES_HOME`/`IRONHERMES_ROOT_HOME`, which would otherwise race under
//! threaded `cargo test` within this same test binary.

#![cfg(feature = "rusty-vault")]

use std::sync::OnceLock;

use ironhermes_core::{
    Config, CustomProviderConfig, IRONHERMES_ROOT_HOME_ENV, ModelsCache, ProviderConfig,
    ProviderResolver, get_hermes_home, get_root_hermes_home, resolve_vault_config,
};
use ironhermes_vault::{RustyVaultConfig, RustyVaultStore, open_store};

/// `tokio::sync::Mutex`, not `std::sync::Mutex` — every test body below holds
/// this guard across `.await` points (vault open/put/get), and a std mutex
/// guard held across `.await` is a clippy `await_holding_lock` hard error
/// under this workspace's `-D warnings` CI gate.
async fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(())).lock().await
}

/// RAII guard: sets `IRONHERMES_HOME` (+ optionally `IRONHERMES_ROOT_HOME`)
/// for the test body, restoring whatever was there before (including "was
/// unset") on drop.
struct EnvPivotGuard {
    prev_home: Option<String>,
    prev_root: Option<String>,
}

impl EnvPivotGuard {
    fn set(home: &std::path::Path, root: Option<&std::path::Path>) -> Self {
        let prev_home = std::env::var("IRONHERMES_HOME").ok();
        let prev_root = std::env::var(IRONHERMES_ROOT_HOME_ENV).ok();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", home);
            match root {
                Some(r) => std::env::set_var(IRONHERMES_ROOT_HOME_ENV, r),
                None => std::env::remove_var(IRONHERMES_ROOT_HOME_ENV),
            }
        }
        Self { prev_home, prev_root }
    }
}

impl Drop for EnvPivotGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.prev_home {
                Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                None => std::env::remove_var("IRONHERMES_HOME"),
            }
            match &self.prev_root {
                Some(v) => std::env::set_var(IRONHERMES_ROOT_HOME_ENV, v),
                None => std::env::remove_var(IRONHERMES_ROOT_HOME_ENV),
            }
        }
    }
}

/// A config with `vault.enabled: true` (empty `data_dir` sentinel — the exact
/// state every production call site starts from), the three built-ins
/// disabled (noise reduction, mirrors `provider_vault_fallback.rs`'s own
/// fixture helper), a main provider whose key is pre-installed (simulating
/// the socket bootstrap having already run — D-07 precedence, priorities 1-4
/// must win and the vault must never even be consulted for it), and a
/// keyless SIBLING provider — the one that trips `apply_vault_fallback`'s
/// per-endpoint loop and is the actual trigger for F-04's crash.
fn config_with_keyless_sibling_provider() -> Config {
    let mut config = Config::default();
    for name in ["openrouter", "anthropic", "openai"] {
        config.providers.insert(
            name.to_string(),
            ProviderConfig {
                disabled: Some(true),
                ..Default::default()
            },
        );
    }
    config.model.provider = "f04-main".to_string();
    config.custom_providers.push(CustomProviderConfig {
        name: "f04-main".to_string(),
        base_url: "https://f04-fixture.example/v1".to_string(),
        api_key: Some("preinstalled-via-socket-bootstrap".to_string()),
        api_mode: None,
        default_model: None,
    });
    config.custom_providers.push(CustomProviderConfig {
        name: "f04-keyless-sibling".to_string(),
        base_url: "https://f04-fixture.example/v1".to_string(),
        api_key: None,
        api_mode: None,
        default_model: None,
    });
    config.vault.enabled = true;
    config.vault.backend = "rusty-vault".to_string();
    config
}

// ---------------------------------------------------------------------------
// GREEN: the fix in effect — root-home stash present.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn profile_pivoted_worker_with_keyless_provider_does_not_die() {
    let _lock = env_lock().await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let root_home = tmp.path().join("root-home");
    let profile_home = root_home.join("profiles").join("f04profile");
    std::fs::create_dir_all(&root_home).expect("mkdir root_home");
    std::fs::create_dir_all(&profile_home).expect("mkdir profile_home");

    // Init a REAL vault at ROOT — the ONE vault this system creates.
    let root_vault_dir = root_home.join("vault");
    let rv_config = RustyVaultConfig {
        data_dir: root_vault_dir.clone(),
        unseal_mode: "keyfile".to_string(),
    };
    RustyVaultStore::init(&rv_config).expect("vault init at root");

    // Mirror resolve_and_set_profile's post-pivot state EXACTLY:
    // IRONHERMES_HOME now the profile dir, IRONHERMES_ROOT_HOME the stashed
    // pre-pivot root.
    let _env_guard = EnvPivotGuard::set(&profile_home, Some(&root_home));

    // Precondition: IRONHERMES_HOME really is pivoted.
    assert_eq!(
        get_hermes_home(),
        profile_home,
        "precondition: IRONHERMES_HOME must be pivoted to the profile dir"
    );
    // The fix under test: get_root_hermes_home() reports the stashed ROOT,
    // not the pivoted IRONHERMES_HOME.
    assert_eq!(
        get_root_hermes_home(),
        root_home,
        "get_root_hermes_home must resolve the stashed root, not the pivoted IRONHERMES_HOME"
    );

    let config = config_with_keyless_sibling_provider();
    assert!(
        config.vault.rusty_vault.data_dir.as_os_str().is_empty(),
        "precondition: data_dir sentinel must be empty"
    );

    // The fix under test: resolve_vault_config() must resolve the sentinel
    // against ROOT, not the pivoted IRONHERMES_HOME.
    let resolved = resolve_vault_config(&config);
    assert_eq!(
        resolved.rusty_vault.data_dir, root_vault_dir,
        "sentinel must resolve to the ROOT vault, not a nonexistent profile-local one (F-04)"
    );

    // open_store must succeed — pre-fix this would have targeted
    // profile_home.join("vault"), which is never created, and failed
    // NotInitialized.
    let store = open_store(&resolved).expect("open_store must succeed against the real root vault");

    // apply_vault_fallback must NOT hard-error for the keyless sibling
    // provider — pre-fix, opening the (nonexistent) profile-local vault
    // failed and propagated via `?`, killing the worker AFTER its main
    // provider's credential had already been installed via the socket
    // bootstrap.
    let mut resolver = ProviderResolver::build_with_cache(&config, ModelsCache::default())
        .expect("resolver build should succeed for the fixture config");
    resolver
        .apply_vault_fallback(&*store)
        .await
        .expect(
            "apply_vault_fallback must succeed for a keyless provider once the sentinel \
             resolves to the real root vault — this is the exact crash F-04 describes",
        );

    // Prove the main provider's pre-installed key (the socket-bootstrap
    // simulation) really did win over the vault per D-07 precedence — the
    // vault was never even consulted for it.
    assert_eq!(
        resolver.resolve("f04-main").unwrap().api_key.as_deref(),
        Some("preinstalled-via-socket-bootstrap"),
        "the pre-installed (socket-bootstrapped) key must never be overwritten by the vault"
    );
}

// ---------------------------------------------------------------------------
// RED (negative control): identical setup, root-home stash absent —
// reproduces F-04's exact original failure.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn without_root_home_stash_the_sentinel_resolves_to_the_pivoted_profile_and_fails() {
    let _lock = env_lock().await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let root_home = tmp.path().join("root-home");
    let profile_home = root_home.join("profiles").join("f04profile");
    std::fs::create_dir_all(&root_home).expect("mkdir root_home");
    std::fs::create_dir_all(&profile_home).expect("mkdir profile_home");

    let root_vault_dir = root_home.join("vault");
    let rv_config = RustyVaultConfig {
        data_dir: root_vault_dir,
        unseal_mode: "keyfile".to_string(),
    };
    RustyVaultStore::init(&rv_config).expect("vault init at root");

    // Pivoted, but IRONHERMES_ROOT_HOME never set — reproduces every call
    // site before this fix, and any future regression that removes the
    // stash in `resolve_and_set_profile`.
    let _env_guard = EnvPivotGuard::set(&profile_home, None);

    let config = config_with_keyless_sibling_provider();
    let resolved = resolve_vault_config(&config);
    assert_eq!(
        resolved.rusty_vault.data_dir,
        profile_home.join("vault"),
        "without the root-home stash, the sentinel falls back to the CURRENT (pivoted) \
         home — this is the exact F-04 bug shape"
    );

    let err = open_store(&resolved)
        .err()
        .expect("opening the nonexistent profile-local vault must fail");
    assert!(
        err.to_string().to_lowercase().contains("not initialized"),
        "expected a NotInitialized-flavored error (F-04's exact crash signature), got: {err}"
    );
}

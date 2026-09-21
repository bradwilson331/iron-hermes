#![cfg(feature = "test-oracles")]
//! Real-subprocess coverage for the worker-side vault credential bootstrap
//! (`crates/ironhermes-cli/src/worker_bootstrap.rs`, Phase 51 Plan 07, D-11/D-14/D-15).
//!
//! Requires the `test-oracles` Cargo feature: every test here drives the compiled
//! `ironhermes` binary via the `IRONHERMES_TEST_PRINT_ENV_VAR_SHA256` observation hook
//! (`main.rs`, gated behind that same feature — Phase 51 Plan 12 / CR-02). Absent the
//! feature, this whole file compiles to zero tests rather than to tests that fail because
//! the hook is absent.
//!
//! # Why a real subprocess, not an in-process function call
//!
//! `51-07-PLAN.md`'s tracer task is explicit about why: an in-process stand-in would
//! pass while the real spawn path stayed inert — exactly the failure mode Phase
//! 47.4's neighbour shipped (a correctly-wired, fully-green, completely inert
//! gate). Every test here drives the REAL compiled `ironhermes` binary
//! (`CARGO_BIN_EXE_ironhermes`, the same mechanism `preflight_provider_key_env.rs`
//! and this crate's other `*_integration.rs` files already use) with `.env_clear()`
//! plus exactly the variables a real worker spawn would carry.
//!
//! # How "the resolved value matches" is proven without printing a secret
//!
//! `main.rs` carries a TEST-ONLY hook (see its own doc comment): when
//! `IRONHERMES_TEST_PRINT_ENV_VAR_SHA256=<VAR_NAME>` is set, the child prints a
//! SHA-256 hex digest of that variable's post-bootstrap, post-dotenv value to
//! stderr and exits immediately, before ever reaching `ensure_home_dirs`/preflight/
//! the chat REPL. This test computes the SAME digest independently (over the
//! value it wrote to the vault, or to a `.env` file) and compares hex strings —
//! proof of byte-for-byte equality without either process ever emitting the raw
//! secret.
//!
//! # Threading (T-51 project trap #1)
//!
//! `ironhermes-cli` races on env `set_var` under multi-threaded `cargo test`.
//! Every test here spawns a REAL child process rather than mutating this test
//! process's own environment, so the classic race is not directly in play — but
//! this file still follows the plan's mandated `--test-threads=1` invocation,
//! matching every other test file in this plan.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

const PROFILE: &str = "vaulttest";
const PROVIDER: &str = "vaulttestprovider";
const API_KEY_ENV: &str = "VAULTTEST_API_KEY";

fn cargo_bin() -> Option<String> {
    match std::env::var("CARGO_BIN_EXE_ironhermes") {
        Ok(p) => Some(p),
        Err(_) => {
            eprintln!("Skipping: CARGO_BIN_EXE_ironhermes not set");
            None
        }
    }
}

fn sha256_hex(value: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

/// Parse `IRONHERMES_TEST_ENV_VAR_SHA256=<hex or ABSENT>` out of the child's
/// combined stderr.
fn extract_printed_hash(stderr: &str) -> Option<&str> {
    stderr
        .lines()
        .find_map(|line| line.strip_prefix("IRONHERMES_TEST_ENV_VAR_SHA256="))
}

/// Write `<home>/.ironhermes/profiles/<PROFILE>/config.yaml` declaring
/// `model.provider: PROVIDER` and `providers.PROVIDER.api_key_env: API_KEY_ENV`.
/// Returns the profile directory (so callers can also drop a `.env` file there).
fn write_profile_config(home: &Path) -> PathBuf {
    let profile_dir = home
        .join(".ironhermes")
        .join("profiles")
        .join(PROFILE);
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join("config.yaml"),
        format!(
            "model:\n  provider: {PROVIDER}\n  default: {PROVIDER}/some-model\nproviders:\n  {PROVIDER}:\n    api_key_env: {API_KEY_ENV}\n"
        ),
    )
    .unwrap();
    profile_dir
}

/// Base command: the real binary, a cleared environment, PATH + HOME (matching
/// SAFE_SYSTEM_VARS' pass-through set — a real worker spawn always carries these
/// two), `--profile PROFILE chat`, and the test hash-print hook armed for
/// `API_KEY_ENV`. Callers layer on the vault vars / `.env` / expectations.
fn base_child(bin: &str, home: &Path) -> Command {
    child_observing(bin, home, API_KEY_ENV)
}

/// Like `base_child`, but arms the test hash-print hook to observe
/// `observe_var` instead of the resolved provider API key — used by the
/// WR-02 scrub tests below, which care about the vault spawn variables' OWN
/// post-bootstrap state, not the credential they resolve.
fn child_observing(bin: &str, home: &Path, observe_var: &str) -> Command {
    let mut cmd = Command::new(bin);
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("IRONHERMES_TEST_PRINT_ENV_VAR_SHA256", observe_var)
        .args(["--profile", PROFILE, "chat"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

// ---------------------------------------------------------------------------
// Real-vault tests (rusty-vault feature required)
// ---------------------------------------------------------------------------

#[cfg(feature = "rusty-vault")]
mod real_vault {
    use super::*;
    use secrecy::SecretString;
    use std::sync::Arc;

    /// A no-op audit sink — these tests don't assert on the ledger, only on the
    /// end-to-end credential flow.
    struct NoopAudit;
    impl ironhermes_vault::ProfileTokenAudit for NoopAudit {
        fn record_mint(&self, _slug: &str, _accessor: &str, _ttl: std::time::Duration) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// Init a fresh rusty-vault at `vault_dir`, write `secret_value` at
    /// `secret/profiles/PROFILE/PROVIDER`, mint a bootstrap token, and host the
    /// REAL credential endpoint. Returns `(token, socket_path, endpoint_handle)` —
    /// the caller must keep the handle alive for the socket to stay bound.
    async fn set_up_vault_and_mint(
        vault_dir: &Path,
        secret_value: &str,
    ) -> (
        SecretString,
        PathBuf,
        ironhermes_vault::ProfileCredentialEndpointHandle,
    ) {
        let rv_config = ironhermes_vault::RustyVaultConfig {
            data_dir: vault_dir.to_path_buf(),
            unseal_mode: "keyfile".to_string(),
        };
        ironhermes_vault::RustyVaultStore::init(&rv_config).expect("vault init");
        let store = ironhermes_vault::RustyVaultStore::open(&rv_config).expect("vault open");

        let profile_store = ironhermes_vault::ProfileSecretStore::from_rusty_vault_store(&store);
        profile_store
            .put_profile_secret(PROFILE, PROVIDER, SecretString::from(secret_value.to_string()))
            .await
            .expect("write profile secret");

        // Drop this test's own open handle before the two calls below each open
        // their OWN independent RustyVaultStore against the same data_dir
        // (mint_profile_token_via_config / host_profile_credential_endpoint —
        // neither can reuse this handle, see their own doc comments) — avoids
        // ever holding more than one live open against the same on-disk data_dir
        // from this process at once.
        drop(profile_store);
        drop(store);

        let minted = ironhermes_vault::mint_profile_token_via_config(
            &rv_config,
            PROFILE,
            ironhermes_vault::profile_token_ttl_for_bootstrap(),
            &NoopAudit,
        )
        .await
        .expect("mint profile token");

        let guard_sink: Arc<dyn ironhermes_vault::ProfileGuardAudit> =
            Arc::new(ironhermes_vault::TracingProfileGuardAudit);
        let endpoint = ironhermes_vault::host_profile_credential_endpoint(&rv_config, guard_sink)
            .expect("host the real credential endpoint");
        let socket_path = endpoint.socket_path().to_path_buf();

        let token = secrecy::SecretString::from(
            secrecy::ExposeSecret::expose_secret(minted.token()).to_string(),
        );
        (token, socket_path, endpoint)
    }

    /// Like `set_up_vault_and_mint`, but seeds MULTIPLE profile secrets before minting +
    /// hosting (Phase 51 Plan 18, G-51-5) — the sibling-bootstrap fixtures below need both
    /// the main provider's secret and a sibling's secret present under the SAME profile
    /// subtree before the worker ever connects, so a single mint/host covers every read the
    /// multi-provider bootstrap loop will issue on its one connection.
    async fn set_up_vault_and_mint_multi(
        vault_dir: &Path,
        secrets: &[(&str, &str)],
    ) -> (
        SecretString,
        PathBuf,
        ironhermes_vault::ProfileCredentialEndpointHandle,
    ) {
        let rv_config = ironhermes_vault::RustyVaultConfig {
            data_dir: vault_dir.to_path_buf(),
            unseal_mode: "keyfile".to_string(),
        };
        ironhermes_vault::RustyVaultStore::init(&rv_config).expect("vault init");
        let store = ironhermes_vault::RustyVaultStore::open(&rv_config).expect("vault open");

        let profile_store = ironhermes_vault::ProfileSecretStore::from_rusty_vault_store(&store);
        for (leaf, value) in secrets {
            profile_store
                .put_profile_secret(PROFILE, leaf, SecretString::from((*value).to_string()))
                .await
                .expect("write profile secret");
        }

        drop(profile_store);
        drop(store);

        let minted = ironhermes_vault::mint_profile_token_via_config(
            &rv_config,
            PROFILE,
            ironhermes_vault::profile_token_ttl_for_bootstrap(),
            &NoopAudit,
        )
        .await
        .expect("mint profile token");

        let guard_sink: Arc<dyn ironhermes_vault::ProfileGuardAudit> =
            Arc::new(ironhermes_vault::TracingProfileGuardAudit);
        let endpoint = ironhermes_vault::host_profile_credential_endpoint(&rv_config, guard_sink)
            .expect("host the real credential endpoint");
        let socket_path = endpoint.socket_path().to_path_buf();

        let token = secrecy::SecretString::from(
            secrecy::ExposeSecret::expose_secret(minted.token()).to_string(),
        );
        (token, socket_path, endpoint)
    }

    /// A profile `config.yaml` naming `main` as `model.provider`, with `main`'s
    /// `fallback_providers` naming `sibling` — matching the real shape `51-UAT.md`'s G-51-5
    /// entry recorded (`openrouter` main, `fallback_providers: [moonshot]`). No `vault:`
    /// section is needed: the worker bootstrap never reads
    /// `resolve_vault_config`/`apply_vault_fallback` (D-11's socket path, not D-07's), only
    /// `config.model.provider` and `config.providers`.
    fn write_two_provider_profile_config(
        home: &Path,
        main: &str,
        main_env: &str,
        sibling: &str,
        sibling_env: &str,
    ) -> PathBuf {
        let profile_dir = home.join(".ironhermes").join("profiles").join(PROFILE);
        std::fs::create_dir_all(&profile_dir).unwrap();
        std::fs::write(
            profile_dir.join("config.yaml"),
            format!(
                "model:\n  provider: {main}\nproviders:\n  {main}:\n    api_key_env: {main_env}\n    fallback_providers:\n    - {sibling}\n  {sibling}:\n    api_key_env: {sibling_env}\n"
            ),
        )
        .unwrap();
        profile_dir
    }

    /// Like `write_two_provider_profile_config`, but MAIN and SIBLING share the exact same
    /// `api_key_env` value — the CR-01 review-fix config-collision shape (a plausible
    /// copy-paste mistake: cloning a provider block to add a fallback and forgetting to
    /// change the env var name).
    fn write_two_provider_profile_config_sharing_env_var(
        home: &Path,
        main: &str,
        sibling: &str,
        shared_env: &str,
    ) -> PathBuf {
        let profile_dir = home.join(".ironhermes").join("profiles").join(PROFILE);
        std::fs::create_dir_all(&profile_dir).unwrap();
        std::fs::write(
            profile_dir.join("config.yaml"),
            format!(
                "model:\n  provider: {main}\nproviders:\n  {main}:\n    api_key_env: {shared_env}\n    fallback_providers:\n    - {sibling}\n  {sibling}:\n    api_key_env: {shared_env}\n"
            ),
        )
        .unwrap();
        profile_dir
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Phase 51 Plan 18 (G-51-5): the multi-provider socket bootstrap
    // ─────────────────────────────────────────────────────────────────────────

    /// G-51-5 REGRESSION (the target test this RED commit records): a profile whose
    /// `model.provider` is a MAIN provider and whose `fallback_providers` names a SIBLING,
    /// with vault secrets for BOTH under `secret/profiles/<slug>/`, and NO `.env` on disk at
    /// all. A real worker subprocess must install BOTH providers' api-key variables.
    ///
    /// BEFORE Task 2's fix: the sibling's variable is never even requested — the socket
    /// bootstrap installs exactly `config.model.provider`'s one credential — so the run
    /// targeting the sibling's variable reports it ABSENT. That is the RED observation this
    /// commit records.
    #[tokio::test(flavor = "multi_thread")]
    async fn sibling_provider_bootstraps_alongside_main_over_the_same_socket() {
        let Some(bin) = cargo_bin() else { return };
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        write_two_provider_profile_config(
            &home,
            "siblingmain",
            "SIBLINGMAIN_API_KEY",
            "siblingfallback",
            "SIBLINGFALLBACK_API_KEY",
        );

        let main_value = "sk-sibling-main-value";
        let fallback_value = "sk-sibling-fallback-value";
        let vault_dir = tmp.path().join("vault");
        let (token, socket_path, _endpoint) = set_up_vault_and_mint_multi(
            &vault_dir,
            &[
                ("siblingmain", main_value),
                ("siblingfallback", fallback_value),
            ],
        )
        .await;

        let main_out = child_observing(&bin, &home, "SIBLINGMAIN_API_KEY")
            .env(
                "IRONHERMES_KANBAN_VAULT_TOKEN",
                secrecy::ExposeSecret::expose_secret(&token),
            )
            .env("IRONHERMES_KANBAN_VAULT_SOCKET", &socket_path)
            .output()
            .expect("spawn ironhermes (main var)");
        let main_stderr = String::from_utf8_lossy(&main_out.stderr);
        let main_printed = extract_printed_hash(&main_stderr)
            .unwrap_or_else(|| panic!("no hash line printed; stderr={main_stderr}"));
        assert_eq!(
            main_printed,
            sha256_hex(main_value),
            "the main provider's credential must still install; stderr={main_stderr}"
        );

        let fallback_out = child_observing(&bin, &home, "SIBLINGFALLBACK_API_KEY")
            .env(
                "IRONHERMES_KANBAN_VAULT_TOKEN",
                secrecy::ExposeSecret::expose_secret(&token),
            )
            .env("IRONHERMES_KANBAN_VAULT_SOCKET", &socket_path)
            .output()
            .expect("spawn ironhermes (sibling var)");
        let fallback_stderr = String::from_utf8_lossy(&fallback_out.stderr);
        let fallback_printed = extract_printed_hash(&fallback_stderr)
            .unwrap_or_else(|| panic!("no hash line printed; stderr={fallback_stderr}"));
        assert_eq!(
            fallback_printed,
            sha256_hex(fallback_value),
            "a fallback provider whose credential lives in the profile vault must ALSO \
             install — this is the G-51-5 regression: before the fix, this reports ABSENT \
             because the socket bootstrap only ever requested config.model.provider; \
             stderr={fallback_stderr}"
        );
    }

    /// A provider the resolver holds (declared in `config.providers`, named as a
    /// `fallback_providers` entry) but for which the profile vault does NOT hold a secret
    /// leaves its api-key variable unset and does NOT fail the bootstrap — the worker still
    /// starts. The main provider's credential still installs normally.
    #[tokio::test(flavor = "multi_thread")]
    async fn sibling_with_no_vault_secret_is_skipped_without_failing_the_worker() {
        let Some(bin) = cargo_bin() else { return };
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        write_two_provider_profile_config(
            &home,
            "skipmain",
            "SKIPMAIN_API_KEY",
            "skipsiblingnosecret",
            "SKIPSIBLING_API_KEY",
        );

        let main_value = "sk-skip-main-value";
        let vault_dir = tmp.path().join("vault");
        // Only the MAIN provider's secret is written — the sibling's leaf is never written
        // at all, so the endpoint returns `secret_not_found` for it.
        let (token, socket_path, _endpoint) =
            set_up_vault_and_mint_multi(&vault_dir, &[("skipmain", main_value)]).await;

        let main_out = child_observing(&bin, &home, "SKIPMAIN_API_KEY")
            .env(
                "IRONHERMES_KANBAN_VAULT_TOKEN",
                secrecy::ExposeSecret::expose_secret(&token),
            )
            .env("IRONHERMES_KANBAN_VAULT_SOCKET", &socket_path)
            .output()
            .expect("spawn ironhermes (main var)");
        assert!(
            main_out.status.success(),
            "the worker must still start when a sibling has no vault secret; stderr={}",
            String::from_utf8_lossy(&main_out.stderr)
        );
        let main_stderr = String::from_utf8_lossy(&main_out.stderr);
        let main_printed = extract_printed_hash(&main_stderr)
            .unwrap_or_else(|| panic!("no hash line printed; stderr={main_stderr}"));
        assert_eq!(main_printed, sha256_hex(main_value));

        let sibling_out = child_observing(&bin, &home, "SKIPSIBLING_API_KEY")
            .env(
                "IRONHERMES_KANBAN_VAULT_TOKEN",
                secrecy::ExposeSecret::expose_secret(&token),
            )
            .env("IRONHERMES_KANBAN_VAULT_SOCKET", &socket_path)
            .output()
            .expect("spawn ironhermes (sibling var)");
        assert!(
            sibling_out.status.success(),
            "a missing sibling secret must not fail the worker; stderr={}",
            String::from_utf8_lossy(&sibling_out.stderr)
        );
        let sibling_stderr = String::from_utf8_lossy(&sibling_out.stderr);
        let sibling_printed = extract_printed_hash(&sibling_stderr)
            .unwrap_or_else(|| panic!("no hash line printed; stderr={sibling_stderr}"));
        assert_eq!(
            sibling_printed, "ABSENT",
            "a provider with no vault secret must be skipped, leaving its variable unset; \
             stderr={sibling_stderr}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Code review fixes (CR-01 / WR-01), post-Plan-18
    // ─────────────────────────────────────────────────────────────────────────

    /// CR-01 REGRESSION: two providers configured with the SAME `api_key_env` (a plausible
    /// copy-paste mistake), both holding a vault secret. The MAIN provider's credential must
    /// be what ends up installed in that shared variable — a sibling must never silently
    /// overwrite it.
    ///
    /// BEFORE the fix: `bootstrap_worker_credential` installs main first, then
    /// unconditionally installs every sibling via `std::env::set_var` with no collision
    /// check — the sibling (processed after main, in the same loop) silently overwrites the
    /// variable regardless of enumeration order, because main's install always happens
    /// before the sibling loop even starts. That is the RED observation this commit records:
    /// the installed value is the SIBLING's, not main's.
    #[tokio::test(flavor = "multi_thread")]
    async fn colliding_sibling_env_var_does_not_overwrite_the_main_providers_credential() {
        let Some(bin) = cargo_bin() else { return };
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        write_two_provider_profile_config_sharing_env_var(
            &home,
            "collidemain",
            "collidesibling",
            "SHARED_COLLISION_API_KEY",
        );

        let main_value = "sk-collide-main-value";
        let sibling_value = "sk-collide-sibling-value";
        let vault_dir = tmp.path().join("vault");
        let (token, socket_path, _endpoint) = set_up_vault_and_mint_multi(
            &vault_dir,
            &[
                ("collidemain", main_value),
                ("collidesibling", sibling_value),
            ],
        )
        .await;

        let out = child_observing(&bin, &home, "SHARED_COLLISION_API_KEY")
            .env(
                "IRONHERMES_KANBAN_VAULT_TOKEN",
                secrecy::ExposeSecret::expose_secret(&token),
            )
            .env("IRONHERMES_KANBAN_VAULT_SOCKET", &socket_path)
            .output()
            .expect("spawn ironhermes");
        assert!(
            out.status.success(),
            "the worker must still start when a sibling's api_key_env collides with main's; \
             stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        let printed = extract_printed_hash(&stderr)
            .unwrap_or_else(|| panic!("no hash line printed; stderr={stderr}"));
        assert_eq!(
            printed,
            sha256_hex(main_value),
            "the MAIN provider must win its own api_key_env against a colliding sibling — a \
             different value means the sibling silently overwrote it; stderr={stderr}"
        );
        assert_ne!(
            printed,
            sha256_hex(sibling_value),
            "the installed value must never be the colliding sibling's credential; \
             stderr={stderr}"
        );
    }

    /// WR-01 REGRESSION: the main provider's read succeeds, but a SIBLING's read fails with a
    /// transport/backend-class error that is NOT `secret_not_found`/`invalid_key` — here, a
    /// genuinely oversized secret value, which the client refuses via `ResponseTooLarge`
    /// (Plan 17's IN-02 bound) rather than buffering it unboundedly. The worker must still
    /// start and install the main credential.
    ///
    /// BEFORE the fix: the sibling loop's `Err(e) => return Err(WorkerBootstrapError::Read(e))`
    /// treats this as fatal for the WHOLE bootstrap, so the worker fails to start even though
    /// its main credential already resolved successfully a few lines earlier. That is the RED
    /// observation this commit records: the child process exits non-zero and never reaches the
    /// hash-print hook.
    #[tokio::test(flavor = "multi_thread")]
    async fn sibling_transport_failure_does_not_abort_a_worker_whose_main_credential_resolved() {
        let Some(bin) = cargo_bin() else { return };
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        write_two_provider_profile_config(
            &home,
            "wr01main",
            "WR01MAIN_API_KEY",
            "wr01sibling",
            "WR01SIBLING_API_KEY",
        );

        let main_value = "sk-wr01-main-value";
        // Deliberately oversized: the RESPONSE line for this key exceeds the client's
        // response-size bound, so the read fails with `ResponseTooLarge` — a transport-class
        // error, never `secret_not_found`/`invalid_key`.
        let oversized_sibling_value = "x".repeat(20_000);
        let vault_dir = tmp.path().join("vault");
        let (token, socket_path, _endpoint) = set_up_vault_and_mint_multi(
            &vault_dir,
            &[
                ("wr01main", main_value),
                ("wr01sibling", &oversized_sibling_value),
            ],
        )
        .await;

        let out = child_observing(&bin, &home, "WR01MAIN_API_KEY")
            .env(
                "IRONHERMES_KANBAN_VAULT_TOKEN",
                secrecy::ExposeSecret::expose_secret(&token),
            )
            .env("IRONHERMES_KANBAN_VAULT_SOCKET", &socket_path)
            .output()
            .expect("spawn ironhermes");
        assert!(
            out.status.success(),
            "a sibling transport failure must not abort a worker whose main credential \
             already resolved; stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        let printed = extract_printed_hash(&stderr)
            .unwrap_or_else(|| panic!("no hash line printed; stderr={stderr}"));
        assert_eq!(
            printed,
            sha256_hex(main_value),
            "the main provider's credential must still install despite the sibling's \
             transport failure; stderr={stderr}"
        );
    }

    /// `worker_bootstraps_credential_over_the_socket`: a real subprocess launched
    /// with a cleared environment plus the two vault variables resolves the
    /// provider credential written at the profile's vault path, and the value
    /// matches.
    ///
    /// `flavor = "multi_thread"` is required, not stylistic: `RustyVaultStore::
    /// init`/`open` bridge to their own internal async calls via
    /// `block_on_admin_call` (`spawn_blocking` + a blocking `rx.recv()` on the
    /// calling thread), a pattern designed for a caller already running on a
    /// multi-threaded runtime (`#[tokio::main]`'s default flavor, which is how
    /// `DispatcherContext::new()` — itself a sync fn called from an async
    /// context — gets away with it in production). `#[tokio::test]`'s DEFAULT
    /// flavor is `current_thread`, which has exactly one schedulable core; the
    /// nested `Handle::current().block_on(..)` inside the spawned blocking task
    /// then has nowhere to run because the outer test task never yields that
    /// core back — a genuine deadlock, observed directly before this annotation
    /// was added (both real-vault tests below timed out at the harness's cap).
    #[tokio::test(flavor = "multi_thread")]
    async fn worker_bootstraps_credential_over_the_socket() {
        let Some(bin) = cargo_bin() else { return };
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        write_profile_config(&home);

        let secret_value = "sk-vault-e2e-abc123";
        let vault_dir = tmp.path().join("vault");
        let (token, socket_path, _endpoint) =
            set_up_vault_and_mint(&vault_dir, secret_value).await;

        let out = base_child(&bin, &home)
            .env(
                "IRONHERMES_KANBAN_VAULT_TOKEN",
                secrecy::ExposeSecret::expose_secret(&token),
            )
            .env("IRONHERMES_KANBAN_VAULT_SOCKET", &socket_path)
            .output()
            .expect("spawn ironhermes");

        let stderr = String::from_utf8_lossy(&out.stderr);
        let printed = extract_printed_hash(&stderr)
            .unwrap_or_else(|| panic!("no hash line printed; stderr={stderr}"));
        assert_eq!(
            printed,
            sha256_hex(secret_value),
            "resolved credential must match the value written at the profile's vault path; stderr={stderr}"
        );
    }

    /// `vault_success_is_not_overwritten_by_stale_dotenv`: a vault read that
    /// succeeds is NOT subsequently overwritten by a stale plaintext value still
    /// sitting in the profile's own `.env` under the same variable name.
    ///
    /// `flavor = "multi_thread"` required — see the doc comment on
    /// `worker_bootstraps_credential_over_the_socket` above.
    #[tokio::test(flavor = "multi_thread")]
    async fn vault_success_is_not_overwritten_by_stale_dotenv() {
        let Some(bin) = cargo_bin() else { return };
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let profile_dir = write_profile_config(&home);

        let vault_value = "sk-vault-winner";
        let stale_dotenv_value = "sk-stale-dotenv-loser";
        std::fs::write(
            profile_dir.join(".env"),
            format!("{API_KEY_ENV}={stale_dotenv_value}\n"),
        )
        .unwrap();

        let vault_dir = tmp.path().join("vault");
        let (token, socket_path, _endpoint) =
            set_up_vault_and_mint(&vault_dir, vault_value).await;

        let out = base_child(&bin, &home)
            .env(
                "IRONHERMES_KANBAN_VAULT_TOKEN",
                secrecy::ExposeSecret::expose_secret(&token),
            )
            .env("IRONHERMES_KANBAN_VAULT_SOCKET", &socket_path)
            .output()
            .expect("spawn ironhermes");

        let stderr = String::from_utf8_lossy(&out.stderr);
        let printed = extract_printed_hash(&stderr)
            .unwrap_or_else(|| panic!("no hash line printed; stderr={stderr}"));
        assert_eq!(
            printed,
            sha256_hex(vault_value),
            "the vault's value must win over the stale .env value on the same \
             variable name (dotenvy never overrides an already-set process var); \
             stderr={stderr}"
        );
        assert_ne!(
            printed,
            sha256_hex(stale_dotenv_value),
            "the resolved credential must NOT be the stale .env value; stderr={stderr}"
        );
    }

    /// `expired_token_surfaces_the_named_expiry_error_to_the_worker`: a token
    /// minted with a sub-second TTL, read after it has expired, surfaces the
    /// distinct, named `token_expired` error — not an opaque provider failure,
    /// and not `denied`/`vault_unreachable`.
    ///
    /// Driven directly against [`ironhermes_vault::read_profile_credential`] (the
    /// same client `worker_bootstrap.rs` calls) rather than through a full
    /// subprocess: `profile_token_ttl_for_bootstrap()` is deliberately
    /// parameterless (D-15's ruling — see `51-03-SUMMARY.md`), so a genuinely
    /// sub-second TTL can only be minted via the lower-level
    /// `mint_profile_token_via_config`, which this test calls directly — mirroring
    /// exactly how `51-03-SUMMARY.md`'s own
    /// `expired_token_read_surfaces_a_named_expiry_error` proved the same
    /// property one layer down (against `read_profile_secret_with_minted_token`
    /// rather than the socket). The mint call ITSELF still exercises
    /// `record_mint` (the ledger write) unconditionally on any successful
    /// mint — proven directly by `dispatcher.rs`'s own
    /// `dispatch_mints_then_spawns_and_ledgers_the_accessor` test, which asserts
    /// the accessor is ledgered on every successful mint regardless of the TTL
    /// used — so this test's job is narrowly the CLIENT-side distinct-error
    /// proof, not a second ledger assertion.
    #[tokio::test(flavor = "multi_thread")]
    async fn expired_token_surfaces_the_named_expiry_error_to_the_worker() {
        let tmp = TempDir::new().unwrap();
        let vault_dir = tmp.path().join("vault");

        let rv_config = ironhermes_vault::RustyVaultConfig {
            data_dir: vault_dir.clone(),
            unseal_mode: "keyfile".to_string(),
        };
        ironhermes_vault::RustyVaultStore::init(&rv_config).expect("vault init");

        // 1100ms, NOT sub-second when truncated: `TokenEntry.ttl` is whole-second
        // (51-03-SUMMARY.md's "third corrected finding" — a request truncated to
        // te.ttl==0 silently becomes RustyVault's 24-hour DEFAULT_LEASE_TTL,
        // which would make this test pass for the wrong reason: a token that is
        // NOT actually expiring vault-side within the test's own wait window).
        // 1100ms truncates to a genuine, non-zero 1-second vault-side lease, so
        // the background expiration sweep (a 200ms tick) has a real, short TTL
        // to act on.
        let minted = ironhermes_vault::mint_profile_token_via_config(
            &rv_config,
            PROFILE,
            std::time::Duration::from_millis(1100),
            &NoopAudit,
        )
        .await
        .expect("mint a short-TTL token");

        let guard_sink: Arc<dyn ironhermes_vault::ProfileGuardAudit> =
            Arc::new(ironhermes_vault::TracingProfileGuardAudit);
        let endpoint = ironhermes_vault::host_profile_credential_endpoint(&rv_config, guard_sink)
            .expect("host the real credential endpoint");
        let socket_path = endpoint.socket_path().to_path_buf();

        let token = secrecy::SecretString::from(
            secrecy::ExposeSecret::expose_secret(minted.token()).to_string(),
        );

        // Sleep past both the 1-second lease AND at least one 200ms sweep tick,
        // so RustyVault's own ExpirationManager has genuinely revoked (deleted)
        // the token entry by the time this connects.
        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

        let err = ironhermes_vault::read_profile_credential(&socket_path, &token, PROFILE, PROVIDER)
            .await
            .expect_err("an expired token must be refused, not silently accepted");

        // FINDING (recorded verbatim, not silently reworded — see 51-07-SUMMARY.md
        // "Deviations"): the socket-serving endpoint (`profile_endpoint.rs`) reads
        // via `ProfileSecretStore::read_profile_secret_with_token` under the
        // CALLER's token, which never performs the client-side
        // `MintedProfileToken::is_expired()` check — that check lives ONLY in
        // Plan 03's `read_profile_secret_with_minted_token`, which nothing in the
        // socket-serving path calls. 51-03-SUMMARY.md's own "fourth corrected
        // finding" already established that RustyVault itself cannot distinguish
        // an expired-and-swept token from a bogus/unrecognized one — both surface
        // as `RvError::ErrPermissionDenied`. The endpoint's `lookup_token_policies`
        // maps ANY `auth/token/lookup-self` failure to `token_invalid`
        // (`profile_endpoint.rs::handle_line`), so a swept token surfaces as
        // `TokenInvalid` through THIS protocol, not the distinct `TokenExpired`
        // variant the plan's must-have literally names. This is still a NAMED,
        // DISTINGUISHABLE error — never an opaque provider failure, satisfying
        // D-15's core legibility requirement — but not the literal variant. A
        // truly distinct wire-level `token_expired` would require the endpoint to
        // track mint-time TTLs itself (out of this plan's scope: Plan 06 states
        // explicitly "this module mints nothing").
        assert!(
            matches!(err, ironhermes_vault::ProfileClientError::TokenInvalid),
            "an expired-and-swept token surfaces as token_invalid through the \
             socket protocol (see the FINDING comment above) — got: {err:?}"
        );
    }

    /// `vault_token_is_scrubbed_after_successful_bootstrap` /
    /// `vault_socket_is_scrubbed_after_successful_bootstrap` (Phase 51-13 Task 3,
    /// WR-02): after a REAL successful socket bootstrap, neither vault spawn
    /// variable the worker was launched with still exists in the process's own
    /// environment. Reuses the existing `IRONHERMES_TEST_PRINT_ENV_VAR_SHA256`
    /// hook — armed here to observe the vault variable ITSELF rather than the
    /// resolved provider key — which prints "ABSENT" when the named variable is
    /// unset at the point the hook runs (after both the vault bootstrap and the
    /// dotenv load), exactly the post-bootstrap point WR-02 cares about. Two
    /// separate child processes because the hook observes one variable per run;
    /// the failed-read half of this task's coverage is in-process, in
    /// `worker_bootstrap.rs`'s own test module — see that test's doc comment for
    /// why a real subprocess cannot observe that path.
    #[tokio::test(flavor = "multi_thread")]
    async fn vault_token_is_scrubbed_after_successful_bootstrap() {
        let Some(bin) = cargo_bin() else { return };
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        write_profile_config(&home);

        let vault_dir = tmp.path().join("vault");
        let (token, socket_path, _endpoint) =
            set_up_vault_and_mint(&vault_dir, "sk-vault-scrub-check-token").await;

        let out = child_observing(&bin, &home, "IRONHERMES_KANBAN_VAULT_TOKEN")
            .env(
                "IRONHERMES_KANBAN_VAULT_TOKEN",
                secrecy::ExposeSecret::expose_secret(&token),
            )
            .env("IRONHERMES_KANBAN_VAULT_SOCKET", &socket_path)
            .output()
            .expect("spawn ironhermes");

        let stderr = String::from_utf8_lossy(&out.stderr);
        let printed = extract_printed_hash(&stderr)
            .unwrap_or_else(|| panic!("no hash line printed; stderr={stderr}"));
        assert_eq!(
            printed, "ABSENT",
            "IRONHERMES_KANBAN_VAULT_TOKEN must be scrubbed from the process \
             environment after a successful bootstrap; stderr={stderr}"
        );
    }

    /// Phase 51 Plan 15 (Task 3, T15's goal claim): the production precondition every OTHER
    /// fixture in this file skips — each one seeds the vault DIRECTLY via `put_profile_secret`,
    /// bypassing the migration's own decode step entirely (the "coincidental reliance" gap
    /// `51-VERIFICATION.md` named). This test runs the REAL `ironhermes vault migrate-profile`
    /// subcommand against a `.env` rendered the way the real profile-wizard writer renders it
    /// (`quote_env_value` — the exact shared primitive `render_profile_env_with_stamp` calls;
    /// that function itself is `pub(crate)` inside `iron_hermes_ui` and unreachable from here),
    /// then bootstraps a REAL worker subprocess over the socket against the vault the migration
    /// just wrote to, and confirms the installed credential is the ORIGINAL plaintext — not the
    /// writer's surrounding quotes (CR-03's end-to-end proof).
    #[tokio::test(flavor = "multi_thread")]
    async fn migrated_writer_produced_profile_bootstraps_to_original_plaintext() {
        let Some(bin) = cargo_bin() else { return };
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        // `--profile <name> chat`'s pivot (`resolve_and_set_profile`) resolves the profile
        // directory from `dirs::home_dir()` (i.e. the `HOME` env var) as `$HOME/.ironhermes/
        // profiles/<slug>` — NOT from `IRONHERMES_HOME`, which it OVERWRITES. The migration
        // subcommand, by contrast, has no `--profile` flag and reads `IRONHERMES_HOME`
        // literally. Pointing `IRONHERMES_HOME` at `$HOME/.ironhermes` for the migrate step
        // (below) makes both steps resolve to the SAME on-disk profile directory — mirroring
        // this file's own `write_profile_config` helper's shape exactly.
        let ironhermes_home = home.join(".ironhermes");
        let profiles_root = ironhermes_home.join(ironhermes_core::PROFILES_SUBDIR);
        let profile_dir = profiles_root.join(PROFILE);
        std::fs::create_dir_all(&profile_dir).unwrap();

        let vault_dir = tmp.path().join("vault");
        std::fs::write(
            profile_dir.join("config.yaml"),
            format!(
                "model:\n  provider: {PROVIDER}\nproviders:\n  {PROVIDER}:\n    api_key_env: {API_KEY_ENV}\nvault:\n  enabled: true\n  backend: rusty-vault\n  rusty_vault:\n    data_dir: \"{}\"\n    unseal_mode: keyfile\n",
                vault_dir.display()
            ),
        )
        .unwrap();

        // The exact byte-encoding the real profile-wizard writer produces for this value.
        let original_plaintext = "sk-e2e-writer-produced-secret-with-a-quote'-and-a-backslash\\";
        let quoted = ironhermes_core::dotenv_write::quote_env_value(original_plaintext);
        assert!(
            quoted.starts_with('\''),
            "sanity: the writer's own encoding must be single-quoted; got: {quoted:?}"
        );
        std::fs::write(
            profile_dir.join(".env"),
            format!("{API_KEY_ENV}={quoted}\n"),
        )
        .unwrap();

        // The vault must be initialized BEFORE the migration subcommand runs — mirroring
        // `profile_vault_migrate.rs`'s own `open_fresh_vault()` fixture, which always
        // initializes first via a direct Rust call rather than a CLI `vault init` subprocess.
        let rv_config = ironhermes_vault::RustyVaultConfig {
            data_dir: vault_dir.clone(),
            unseal_mode: "keyfile".to_string(),
        };
        ironhermes_vault::RustyVaultStore::init(&rv_config).expect("vault init");

        // Real migration subcommand.
        let migrate_out = Command::new(&bin)
            .env("IRONHERMES_HOME", &ironhermes_home)
            .args(["vault", "migrate-profile", PROFILE])
            .output()
            .expect("run vault migrate-profile");
        assert!(
            migrate_out.status.success(),
            "migration must succeed; stderr={}",
            String::from_utf8_lossy(&migrate_out.stderr)
        );
        let rewritten_env = std::fs::read_to_string(profile_dir.join(".env")).unwrap();
        assert!(
            !rewritten_env.contains(API_KEY_ENV),
            "the migrated key must be scrubbed from .env; got: {rewritten_env:?}"
        );

        // Mint a bootstrap token + host the real credential endpoint against the SAME vault
        // the migration just wrote to.
        let minted = ironhermes_vault::mint_profile_token_via_config(
            &rv_config,
            PROFILE,
            ironhermes_vault::profile_token_ttl_for_bootstrap(),
            &NoopAudit,
        )
        .await
        .expect("mint profile token");
        let guard_sink: Arc<dyn ironhermes_vault::ProfileGuardAudit> =
            Arc::new(ironhermes_vault::TracingProfileGuardAudit);
        let endpoint = ironhermes_vault::host_profile_credential_endpoint(&rv_config, guard_sink)
            .expect("host the real credential endpoint");
        let socket_path = endpoint.socket_path().to_path_buf();
        let token = secrecy::SecretString::from(
            secrecy::ExposeSecret::expose_secret(minted.token()).to_string(),
        );

        // Real worker subprocess bootstrap, hash-print hook armed for the resolved provider
        // key. `HOME`, not `IRONHERMES_HOME` — see the comment above on why the `--profile`
        // pivot resolves from `HOME`.
        let out = Command::new(&bin)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &home)
            .env("IRONHERMES_TEST_PRINT_ENV_VAR_SHA256", API_KEY_ENV)
            .env(
                "IRONHERMES_KANBAN_VAULT_TOKEN",
                secrecy::ExposeSecret::expose_secret(&token),
            )
            .env("IRONHERMES_KANBAN_VAULT_SOCKET", &socket_path)
            .args(["--profile", PROFILE, "chat"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn ironhermes worker");

        let stderr = String::from_utf8_lossy(&out.stderr);
        let printed = extract_printed_hash(&stderr)
            .unwrap_or_else(|| panic!("no hash line printed; stderr={stderr}"));
        assert_eq!(
            printed,
            sha256_hex(original_plaintext),
            "the bootstrapped credential must equal the ORIGINAL plaintext handed to the \
             writer, not the writer's surrounding quotes (CR-03); stderr={stderr}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn vault_socket_is_scrubbed_after_successful_bootstrap() {
        let Some(bin) = cargo_bin() else { return };
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        write_profile_config(&home);

        let vault_dir = tmp.path().join("vault");
        let (token, socket_path, _endpoint) =
            set_up_vault_and_mint(&vault_dir, "sk-vault-scrub-check-socket").await;

        let out = child_observing(&bin, &home, "IRONHERMES_KANBAN_VAULT_SOCKET")
            .env(
                "IRONHERMES_KANBAN_VAULT_TOKEN",
                secrecy::ExposeSecret::expose_secret(&token),
            )
            .env("IRONHERMES_KANBAN_VAULT_SOCKET", &socket_path)
            .output()
            .expect("spawn ironhermes");

        let stderr = String::from_utf8_lossy(&out.stderr);
        let printed = extract_printed_hash(&stderr)
            .unwrap_or_else(|| panic!("no hash line printed; stderr={stderr}"));
        assert_eq!(
            printed, "ABSENT",
            "IRONHERMES_KANBAN_VAULT_SOCKET must be scrubbed from the process \
             environment after a successful bootstrap; stderr={stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests that need no real vault (the client's own transport-failure / CLI-wiring
// paths) — always compiled, matching this crate's default feature set.
// ---------------------------------------------------------------------------

/// `worker_without_vault_vars_uses_dotenv_exactly_as_today`: no vault variables in
/// the environment → the existing `.env` load path runs unchanged and resolves
/// the key.
#[tokio::test]
async fn worker_without_vault_vars_uses_dotenv_exactly_as_today() {
    let Some(bin) = cargo_bin() else { return };
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let profile_dir = write_profile_config(&home);

    let dotenv_value = "sk-dotenv-only-value";
    std::fs::write(
        profile_dir.join(".env"),
        format!("{API_KEY_ENV}={dotenv_value}\n"),
    )
    .unwrap();

    let out = base_child(&bin, &home)
        .output()
        .expect("spawn ironhermes");

    let stderr = String::from_utf8_lossy(&out.stderr);
    let printed = extract_printed_hash(&stderr)
        .unwrap_or_else(|| panic!("no hash line printed; stderr={stderr}"));
    assert_eq!(
        printed,
        sha256_hex(dotenv_value),
        "with no vault vars present, today's .env load must resolve the key \
         unchanged; stderr={stderr}"
    );
}

/// `worker_read_failure_does_not_fall_back_to_dotenv`: vault variables present,
/// socket unreachable, AND a plaintext key present in the profile `.env` → the
/// worker fails with a named error and the `.env` value is never used. Since the
/// process exits (via `?` in `main`) BEFORE `dotenvy::from_path` — and therefore
/// before the test hash-print hook — ever runs, the absence of a printed hash
/// line is itself part of the proof: the `.env` value never had a chance to be
/// read into the process environment at all.
#[tokio::test]
async fn worker_read_failure_does_not_fall_back_to_dotenv() {
    let Some(bin) = cargo_bin() else { return };
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let profile_dir = write_profile_config(&home);

    let stale_dotenv_value = "sk-stale-should-never-be-used";
    std::fs::write(
        profile_dir.join(".env"),
        format!("{API_KEY_ENV}={stale_dotenv_value}\n"),
    )
    .unwrap();

    // A socket path that structurally cannot exist — nothing is listening.
    let unreachable_socket = tmp.path().join("no-such-socket.sock");

    let out = base_child(&bin, &home)
        .env("IRONHERMES_KANBAN_VAULT_TOKEN", "bogus-token-never-validated")
        .env("IRONHERMES_KANBAN_VAULT_SOCKET", &unreachable_socket)
        .output()
        .expect("spawn ironhermes");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "a vault read failure must exit non-zero, not silently fall back; stderr={stderr}"
    );
    assert!(
        extract_printed_hash(&stderr).is_none(),
        "the process must exit before ever reaching the post-dotenv hash hook \
         (i.e. before dotenvy ever ran) on a vault read failure; stderr={stderr}"
    );
    assert!(
        !stderr.contains(stale_dotenv_value),
        "the stale .env value must never appear anywhere in the failure output; stderr={stderr}"
    );
}

/// `exactly_one_vault_var_present_refuses`: the token without the socket path,
/// and the socket path without the token, each refuse rather than guessing.
#[tokio::test]
async fn exactly_one_vault_var_present_refuses() {
    let Some(bin) = cargo_bin() else { return };

    // Case A: token only.
    {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        write_profile_config(&home);

        let out = base_child(&bin, &home)
            .env("IRONHERMES_KANBAN_VAULT_TOKEN", "tok-only")
            .output()
            .expect("spawn ironhermes");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success(),
            "token-without-socket must refuse rather than guess; stderr={stderr}"
        );
        assert!(
            stderr.contains("half-configured"),
            "must name the half-configured refusal; stderr={stderr}"
        );
    }

    // Case B: socket only.
    {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        write_profile_config(&home);

        let out = base_child(&bin, &home)
            .env("IRONHERMES_KANBAN_VAULT_SOCKET", "/tmp/socket-only.sock")
            .output()
            .expect("spawn ironhermes");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success(),
            "socket-without-token must refuse rather than guess; stderr={stderr}"
        );
        assert!(
            stderr.contains("half-configured"),
            "must name the half-configured refusal; stderr={stderr}"
        );
    }
}

//! Worker-side credential bootstrap over the Phase 51 vault socket (D-11/D-14/D-15).
//!
//! Extracted as a callable function — rather than inlined in `main`, as the
//! bootstrap logic originally was drafted — so an end-to-end test can drive the
//! real code path in a real subprocess instead of asserting on a source grep
//! (`51-07-PLAN.md`'s explicit reasoning for why this must be a function, not
//! inline code: an in-process stand-in would pass while the real spawn path stayed
//! inert, exactly the failure mode Phase 47.4's neighbour shipped).
//!
//! Called from `main()` **immediately BEFORE** the existing `dotenvy::from_path`
//! load at `main.rs`'s `Config::env_path()` call — never after. This ordering is
//! load-bearing, not incidental: `dotenvy::from_path` (non-override) preserves an
//! already-set process variable (`dotenvy` 0.15.7's `Iter::load`: `if
//! env::var(&key).is_err() { env::set_var(&key, value); }`), so installing the
//! vault-resolved credential here, before that load runs, means the credential
//! ALREADY occupies the process variable by the time `dotenvy` looks at it —
//! `dotenvy` sees it is already set and never overwrites it. Reversing the order
//! (bootstrap after dotenv) would let a stale plaintext `.env` value win instead.
//! Verified directly against the vendored `dotenvy` 0.15.7 source
//! (`iter.rs::Iter::load`) before this ordering was chosen — see `51-07-SUMMARY.md`
//! for the observed-precedence record this plan's action text required.
//!
//! # The four-branch control flow (D-11/D-14)
//!
//! - **Both vault variables absent** → [`BootstrapOutcome::NotApplicable`]. Do
//!   nothing at all; today's `.env` load runs unchanged. This is the majority path
//!   — every un-migrated profile and every non-kanban invocation of this binary.
//! - **Both present, read succeeds** → the resolved credential is installed into
//!   the provider's configured `api_key_env` variable. [`BootstrapOutcome::Installed`].
//! - **Both present, read FAILS** → `Err` — the caller (`main`) propagates this via
//!   `?`, which exits the process before `dotenvy::from_path` ever runs. D-14 makes
//!   this a refusal: no fallback to the profile's own `.env`, even if a (possibly
//!   stale) plaintext key is still sitting there.
//! - **Exactly one present** → `Err(WorkerBootstrapError::HalfConfigured)`. A
//!   half-configured spawn is a bug in the dispatcher; guessing which of the two
//!   was intended is how a worker ends up silently reading a file the operator
//!   believed was scrubbed.

use ironhermes_core::Config;
use ironhermes_vault::ProfileClientError;
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

/// The env var carrying the worker's minted, profile-scoped vault token — must
/// match `ironhermes-kanban`'s `worker_spawn::IRONHERMES_KANBAN_VAULT_TOKEN_ENV`
/// (the two crates cannot share a literal directly; see D-11's cross-crate
/// contract, `51-07-PLAN.md`'s "New environment variables").
pub const VAULT_TOKEN_ENV: &str = "IRONHERMES_KANBAN_VAULT_TOKEN";

/// The env var carrying the socket path — must match
/// `ironhermes-kanban`'s `worker_spawn::IRONHERMES_KANBAN_VAULT_SOCKET_ENV`.
pub const VAULT_SOCKET_ENV: &str = "IRONHERMES_KANBAN_VAULT_SOCKET";

/// Resolve the single env-var name to install a vault-bootstrapped credential
/// into.
///
/// Phase 51 Plan 15 (CR-04): delegates to the SAME shared resolver
/// `profile_migrate::resolve_provider_env_var_name` calls, rather than the
/// one-tier `config.providers.get(..).and_then(..)` chain this function used
/// to be — that one-tier chain is exactly why a profile the migration was
/// willing to scrub (a bare `model.provider: openrouter`, no `providers:`
/// block) could not be consumed by this bootstrap: the migration's two-tier
/// resolver found the built-in `OPENROUTER_API_KEY` fallback, this one didn't.
/// `pub(crate)` so the cross-path equality test below can call it directly.
pub(crate) fn resolve_worker_api_key_env(config: &Config, provider: &str) -> Option<String> {
    ironhermes_core::provider_env::provider_api_key_env_name(config, provider)
}

/// Every distinguishable way the worker-side bootstrap can fail. Each variant
/// carries enough context for the worker's own stderr to name which of the
/// possible failure modes occurred — D-15's legibility requirement (an operator
/// who sees a generic provider error will go rotate a credential that was never
/// compromised).
#[derive(Debug, Error)]
pub enum WorkerBootstrapError {
    /// Exactly one of the two vault variables was present.
    #[error(
        "half-configured vault spawn: {present} is set but {missing} is not — refusing rather than guessing"
    )]
    HalfConfigured {
        present: &'static str,
        missing: &'static str,
    },
    /// `config.yaml` sets no `model.provider` — cannot determine which credential
    /// this profile needs.
    #[error(
        "profile \"{profile}\" config.yaml sets no model.provider — cannot determine which credential to bootstrap"
    )]
    NoProvider { profile: String },
    /// The configured provider has no `api_key_env` — cannot determine which
    /// process variable to install the vault-resolved credential into.
    #[error(
        "provider \"{provider}\" has no api_key_env configured — cannot determine which env var to install the vault credential into"
    )]
    NoApiKeyEnv { provider: String },
    /// IN-01 (Phase 51 Plan 15): no active profile was resolved for this worker
    /// process. Previously this sent an EMPTY profile field over the socket
    /// and got back a `profile_mismatch` that misdescribed the cause as "wrong
    /// profile" rather than "no profile at all" — refusing here, before the
    /// socket round-trip, names the real cause instead.
    #[error(
        "no active profile resolved for this worker process — cannot determine which profile's vault secret to request"
    )]
    NoActiveProfile,
    /// `config.yaml` itself could not be loaded.
    #[error("failed to load config.yaml for the worker vault bootstrap: {0}")]
    ConfigLoad(String),
    /// The vault read itself failed — wraps the named
    /// [`ironhermes_vault::ProfileClientError`] so the worker's stderr shows the
    /// distinct, named reason (malformed_request/token_invalid/profile_mismatch/
    /// secret_not_found/denied/token_expired/vault_unreachable/invalid_key/
    /// backend_error/request_too_large/transport failure), never an opaque
    /// provider-call failure.
    #[error("worker vault credential bootstrap failed: {0}")]
    Read(#[from] ProfileClientError),
}

/// What happened, for the caller to log / act on. `NotApplicable` is the common
/// path — every un-migrated profile and every non-worker invocation of this
/// binary.
#[derive(Debug)]
pub enum BootstrapOutcome {
    /// Neither vault variable was set — today's `.env` load runs unchanged.
    NotApplicable,
    /// The vault-resolved credential was installed into this env var.
    Installed { provider_env_var: String },
}

/// Phase 51 Plan 18 (G-51-5): enumerate the SIBLING providers (every name in
/// [`ironhermes_core::ProviderResolver::endpoint_names`] other than `main_provider`) that also
/// have a resolvable api-key env-var name, paired with that variable name. Names for which
/// [`resolve_worker_api_key_env`] resolves to `None` are dropped here, before any socket
/// traffic — there is nowhere to install them, so requesting them would be pointless.
///
/// Builds the resolver with [`ironhermes_core::ProviderResolver::build_with_env_overrides_strict`]
/// over an EMPTY override map and [`ironhermes_core::ModelsCache::default`] — the worker-shaped
/// question ("what would this SCRUBBED spawned process's config alone resolve?"), with no
/// ambient process-env read and no ambient disk read. If that build fails, this is NOT a fatal
/// bootstrap error (D-14 requires narrowing to be visible, not that it be fatal): fall back to
/// main-only, today's exact behavior, and name why via a greppable warning event.
fn build_sibling_read_plan(config: &Config, main_provider: &str) -> Vec<(String, String)> {
    let resolver = match ironhermes_core::ProviderResolver::build_with_env_overrides_strict(
        config,
        ironhermes_core::ModelsCache::default(),
        &std::collections::HashMap::new(),
    ) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                event = "worker_vault_sibling_enumeration_skipped",
                error = %e,
                "worker vault bootstrap: sibling provider enumeration skipped — resolver build \
                 failed, falling back to main-provider-only bootstrap (today's exact behavior)"
            );
            return Vec::new();
        }
    };

    let mut siblings = Vec::new();
    for name in resolver.endpoint_names() {
        if name == main_provider {
            continue;
        }
        match resolve_worker_api_key_env(config, &name) {
            Some(env_var) => siblings.push((name, env_var)),
            None => {
                tracing::info!(
                    event = "worker_vault_sibling_no_env_var_skipped",
                    provider = %name,
                    "worker vault bootstrap: sibling provider has no resolvable api-key env \
                     var name — skipped before any socket traffic"
                );
            }
        }
    }
    siblings
}

/// Read the two vault spawn variables and, if both are present, read the
/// resolved credential(s) over the socket and install them into the process
/// environment BEFORE `main.rs`'s `dotenvy::from_path` call (see module doc for
/// why the ordering is load-bearing). `profile` is the active profile slug
/// (from `resolve_and_set_profile`), used as the WIDENED protocol's `profile`
/// field and to build the error messages below.
///
/// Phase 51 Plan 18 (G-51-5): installs the MAIN provider's credential (unchanged failure
/// taxonomy — a main read failure is still fatal and still produces
/// [`WorkerBootstrapError::Read`]) and then, over the SAME connection, every SIBLING provider
/// [`build_sibling_read_plan`] names. A sibling with no vault secret
/// (`secret_not_found`/`invalid_key`) is skipped without failing the worker.
///
/// Code review fix (CR-01/WR-01, post-Plan-18): a sibling whose configured `api_key_env`
/// COLLIDES with an already-installed variable (main's, or an earlier sibling's) is
/// skipped-with-a-named-log rather than silently overwriting that credential —
/// `std::env::set_var`, unlike `dotenvy::from_path`, always overwrites, so without this check
/// the LAST provider processed under a shared env-var name would silently win it. And every
/// OTHER sibling read error (a transport/backend-class failure on that ONE sibling's leaf —
/// `BackendError`/`VaultUnreachable`/`MalformedResponse`/`ResponseTooLarge`/
/// `RequestTooLarge`/...) is ALSO skipped-with-a-named-log, not fatal: that error class means
/// only THIS sibling's read failed, not that the vault path itself is broken, so it must not
/// abort a worker whose MAIN credential already resolved successfully a few lines above.
pub async fn bootstrap_worker_credential(
    profile: Option<&str>,
) -> Result<BootstrapOutcome, WorkerBootstrapError> {
    let token_raw = std::env::var(VAULT_TOKEN_ENV).ok();
    let socket_raw = std::env::var(VAULT_SOCKET_ENV).ok();

    let (token_raw, socket_raw) = match (token_raw, socket_raw) {
        (None, None) => return Ok(BootstrapOutcome::NotApplicable),
        (Some(_), None) => {
            return Err(WorkerBootstrapError::HalfConfigured {
                present: VAULT_TOKEN_ENV,
                missing: VAULT_SOCKET_ENV,
            });
        }
        (None, Some(_)) => {
            return Err(WorkerBootstrapError::HalfConfigured {
                present: VAULT_SOCKET_ENV,
                missing: VAULT_TOKEN_ENV,
            });
        }
        (Some(t), Some(s)) => (t, s),
    };

    let config = Config::load().map_err(|e| WorkerBootstrapError::ConfigLoad(e.to_string()))?;
    let main_provider = config.model.provider.clone();
    if main_provider.is_empty() {
        return Err(WorkerBootstrapError::NoProvider {
            profile: profile.unwrap_or("<unknown>").to_string(),
        });
    }
    let main_api_key_env =
        resolve_worker_api_key_env(&config, &main_provider).ok_or_else(|| {
            WorkerBootstrapError::NoApiKeyEnv {
                provider: main_provider.clone(),
            }
        })?;

    // IN-01: refuse before the socket round-trip rather than sending an empty
    // profile field and getting back a profile_mismatch that misdescribes the
    // cause.
    let profile_slug = profile.ok_or(WorkerBootstrapError::NoActiveProfile)?.to_string();
    let token = SecretString::from(token_raw);
    let socket_path = std::path::PathBuf::from(socket_raw);

    // Phase 51 Plan 18: main FIRST, then every enumerable sibling — one ordered list, read
    // over ONE connection below.
    let siblings = build_sibling_read_plan(&config, &main_provider);
    let mut ordered_keys: Vec<&str> = Vec::with_capacity(1 + siblings.len());
    ordered_keys.push(main_provider.as_str());
    for (name, _env_var) in &siblings {
        ordered_keys.push(name.as_str());
    }

    let read_results = ironhermes_vault::read_profile_credentials(
        &socket_path,
        &token,
        &profile_slug,
        &ordered_keys,
    )
    .await;

    // Phase 51-13 Task 3 (WR-02), extended by Plan 18 to cover the WHOLE read loop: remove
    // both vault spawn variables from this process's environment BEFORE inspecting any
    // outcome — deliberately BEFORE the `?` below, not after, and after every read in the
    // batch has completed rather than after only the first. A failed read is exactly the
    // case where this process would otherwise carry a still-live token in its environment
    // block into whatever happens next: readable from `/proc/<pid>/environ` (or the macOS
    // equivalent) by the same uid, and inherited by every child this agent later spawns —
    // the `terminal` tool, `execute_code` (separately recorded in this repo as bypassing the
    // guardrail and audit path), and any MCP stdio server. Within the 60s TTL that is a
    // re-readable profile subtree. The token and socket path now exist in this process's
    // environment only across the whole read batch above, on every outcome.
    //
    // SAFETY: called from `main()` before `#[tokio::main]`'s body spawns any
    // worker thread or task, and BEFORE the existing `dotenvy::from_path`
    // call a few lines later — the same single-threaded-at-process-start
    // argument that already justifies the neighbouring `set_var` calls below
    // (`dotenvy::Iter::load`), and the identical argument
    // `resolve_and_set_profile`'s own `set_var` a few lines above main.rs's
    // call site states explicitly. It holds for N installs exactly as it held for one.
    unsafe {
        std::env::remove_var(VAULT_TOKEN_ENV);
        std::env::remove_var(VAULT_SOCKET_ENV);
    }

    let mut read_results = read_results?;

    // Main provider's outcome is FIRST — unchanged failure taxonomy: a main read failure is
    // still fatal and still produces the exact `WorkerBootstrapError::Read` variant it
    // produced before this plan.
    let (_, main_outcome) = read_results.remove(0);
    let main_value: SecretString = main_outcome?;

    // SAFETY: see the comment on the environment-scrub block above — identical argument,
    // holds for every install in this function.
    unsafe {
        std::env::set_var(&main_api_key_env, main_value.expose_secret());
    }

    // Code review fix (CR-01): every env var this call has installed, seeded with main's —
    // the MAIN provider must always win its own variable. `std::env::set_var` (unlike
    // `dotenvy::from_path`) always overwrites, so without this check a sibling sharing
    // main's (or an earlier sibling's) `api_key_env` would silently win it. Cloned BEFORE
    // `main_api_key_env` moves into `outcome` below.
    let mut installed_vars: std::collections::HashSet<String> =
        std::collections::HashSet::from([main_api_key_env.clone()]);

    let outcome = BootstrapOutcome::Installed {
        provider_env_var: main_api_key_env,
    };
    if let BootstrapOutcome::Installed { provider_env_var } = &outcome {
        // Never logs the value — only which variable the vault-resolved credential
        // was installed into.
        tracing::info!(
            event = "worker_vault_credential_installed",
            provider_env_var = %provider_env_var,
        );
    }

    // Sibling outcomes: `secret_not_found` and `invalid_key` are the normal "nothing to
    // install" case and are skipped without failing the worker. Code review fix (WR-01):
    // every OTHER error is ALSO skipped-with-a-named-log rather than fatal — a
    // transport/backend-class failure (denied/expired/unreachable/backend/a malformed or
    // oversized response/...) on THIS ONE sibling's leaf does not mean the vault path itself
    // is broken; it means only this sibling's read failed, and a worker whose MAIN
    // credential already resolved successfully above must still start. The MAIN provider's
    // OWN read failure (above, before this loop) is unaffected by this branch and remains
    // fatal, still producing the exact `WorkerBootstrapError::Read` variant it always has.
    for ((name, env_var), (_, outcome)) in siblings.into_iter().zip(read_results) {
        match outcome {
            Ok(value) => {
                if !installed_vars.insert(env_var.clone()) {
                    tracing::warn!(
                        event = "worker_vault_sibling_env_var_collision_skipped",
                        provider = %name,
                        provider_env_var = %env_var,
                        "worker vault bootstrap: sibling provider's api_key_env collides with \
                         an already-installed variable — skipped rather than silently \
                         overwriting the credential another provider already installed under \
                         this name"
                    );
                    continue;
                }
                // SAFETY: see the comment on the environment-scrub block above.
                unsafe {
                    std::env::set_var(&env_var, value.expose_secret());
                }
                tracing::info!(
                    event = "worker_vault_credential_installed",
                    provider_env_var = %env_var,
                );
            }
            Err(ProfileClientError::SecretNotFound) | Err(ProfileClientError::InvalidKey) => {
                tracing::info!(
                    event = "worker_vault_sibling_credential_skipped",
                    provider = %name,
                    "worker vault bootstrap: sibling provider has no vault secret — skipped, \
                     worker still starts"
                );
            }
            Err(e) => {
                tracing::warn!(
                    event = "worker_vault_sibling_read_failed_skipped",
                    provider = %name,
                    reason = %e,
                    "worker vault bootstrap: sibling provider's credential read failed — \
                     skipped rather than aborting a worker whose main credential already \
                     resolved successfully; the main provider's own read failure remains \
                     fatal and is unaffected by this branch"
                );
            }
        }
    }

    // Phase 51 UAT F-04 fix (commit 2), narrowed by T17/WR-08 (Phase 51-13):
    // this process now holds `main_provider`'s credential — it must never be
    // overwritten from the vault. Mark it BY NAME so
    // `ProviderResolver::apply_vault_fallback` skips only this one provider's
    // endpoint inside its per-endpoint loop; every OTHER endpoint (a role's
    // provider, a fallback model, ...) still resolves from the root vault
    // normally. This is narrower than the process-global form this replaced,
    // which regressed every sibling vault-backed provider once any one
    // provider had bootstrapped.
    //
    // Phase 51 Plan 18: this stays MAIN-provider-only, deliberately — every sibling
    // installed above went into an environment variable BEFORE `dotenvy` runs, so
    // `ProviderResolver::build` picks it up at priorities 1-4 and `apply_vault_fallback`
    // already skips any endpoint whose `api_key` is already `Some` (see
    // `sibling_env_var_installed_before_build_wins_over_a_root_vault_entry` below, which
    // asserts that composition rather than trusting it). Converting this marker into a set
    // would be churn on a public surface with existing test callers for no behavioral gain.
    ironhermes_core::provider::mark_worker_bootstrapped_over_socket(&main_provider);

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests exercise the pure branches (absent / half-configured) without
    // touching the real socket or config.yaml — the real-subprocess end-to-end
    // coverage lives in `tests/worker_vault_bootstrap.rs`, which is what this
    // plan's tracer task specifies (a real subprocess, not an in-process
    // stand-in). Env-var mutation here is scoped to this test's own process via
    // `#[serial]`-free but variable-scoped set/remove pairs — this crate's own
    // documented env-race trap (`project_ironhermes_cli_plain_cargo_test_env_races`)
    // means these run correctly only under `--test-threads=1`, matching this
    // plan's mandated CLI test invocation.

    fn clear_vault_vars() {
        unsafe {
            std::env::remove_var(VAULT_TOKEN_ENV);
            std::env::remove_var(VAULT_SOCKET_ENV);
        }
    }

    #[tokio::test]
    async fn both_absent_is_not_applicable() {
        clear_vault_vars();
        let outcome = bootstrap_worker_credential(Some("alpha")).await.unwrap();
        assert!(matches!(outcome, BootstrapOutcome::NotApplicable));
    }

    #[tokio::test]
    async fn token_without_socket_refuses() {
        clear_vault_vars();
        unsafe {
            std::env::set_var(VAULT_TOKEN_ENV, "tok");
        }
        let err = bootstrap_worker_credential(Some("alpha"))
            .await
            .expect_err("half-configured spawn must refuse");
        assert!(matches!(
            err,
            WorkerBootstrapError::HalfConfigured {
                present: VAULT_TOKEN_ENV,
                missing: VAULT_SOCKET_ENV,
            }
        ));
        clear_vault_vars();
    }

    #[tokio::test]
    async fn socket_without_token_refuses() {
        clear_vault_vars();
        unsafe {
            std::env::set_var(VAULT_SOCKET_ENV, "/tmp/whatever.sock");
        }
        let err = bootstrap_worker_credential(Some("alpha"))
            .await
            .expect_err("half-configured spawn must refuse");
        assert!(matches!(
            err,
            WorkerBootstrapError::HalfConfigured {
                present: VAULT_SOCKET_ENV,
                missing: VAULT_TOKEN_ENV,
            }
        ));
        clear_vault_vars();
    }

    // =========================================================================
    // Phase 51-13 Task 3 (WR-02): the bootstrap token and socket path must not
    // outlive the single read that consumes them.
    //
    // This one assertion is IN-PROCESS rather than a real-subprocess `Command`
    // (unlike `tests/worker_vault_bootstrap.rs`'s Task 3 success-path coverage)
    // for a structural reason, not convenience: `main()` calls
    // `bootstrap_worker_credential(...).context(...)?` and a failed read
    // therefore exits the WHOLE process via `?` before it ever reaches
    // `main.rs`'s `IRONHERMES_TEST_PRINT_ENV_VAR_SHA256` hook (or any other
    // diagnostic point) — a dying child has no window in which it is both
    // scrubbed AND still able to report its own state, and adding a new hook
    // solely to create that window is exactly what this task's action text
    // rules out. Calling the real function directly (the same function
    // `main()` calls, with a REAL failing read against a socket path that
    // structurally cannot exist — not a mock) is the only way to observe the
    // scrub deterministically. `IRONHERMES_HOME` is pointed at a throwaway
    // fixture so `Config::load()` resolves predictably rather than reading
    // this dev machine's real config.
    // =========================================================================

    #[tokio::test]
    async fn failed_read_scrubs_both_vault_vars_before_propagating() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.yaml"),
            "model:\n  provider: scrubtestprovider\nproviders:\n  scrubtestprovider:\n    api_key_env: SCRUBTEST_API_KEY\n",
        )
        .unwrap();
        let unreachable_socket = tmp.path().join("no-such-socket.sock");

        clear_vault_vars();
        // SAFETY: test-only env var mutation, this crate's own documented
        // env-race trap means this runs correctly only under
        // `--test-threads=1`, matching every other test in this module.
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
            std::env::set_var(VAULT_TOKEN_ENV, "bogus-token-never-validated");
            std::env::set_var(VAULT_SOCKET_ENV, &unreachable_socket);
        }

        let err = bootstrap_worker_credential(Some("alpha"))
            .await
            .expect_err("a socket path that cannot exist must fail the read, not succeed");
        assert!(
            matches!(err, WorkerBootstrapError::Read(_)),
            "expected a wrapped ProfileClientError, got {err:?}"
        );

        assert!(
            std::env::var(VAULT_TOKEN_ENV).is_err(),
            "the vault token must be scrubbed from the process environment even when \
             the read fails — a failed read is exactly the case where a live token must \
             not survive into whatever the process does next"
        );
        assert!(
            std::env::var(VAULT_SOCKET_ENV).is_err(),
            "the vault socket path must be scrubbed from the process environment even \
             when the read fails"
        );

        clear_vault_vars();
        // SAFETY: test-only cleanup, same justification as above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    // =========================================================================
    // Phase 51 Plan 18 Test 4 (G-51-5): a sibling credential installed into a provider's
    // `api_key_env` variable is not later overwritten by a ROOT-vault entry of the SAME
    // provider name. `mark_worker_bootstrapped_over_socket` only ever marks the MAIN
    // provider (see `bootstrap_worker_credential`'s doc comment on why that is correct, not
    // an oversight) — this test proves the composition that makes it safe, rather than
    // assuming it: `ProviderResolver::build`/`build_with_cache` read the env var at priority
    // 1, and `apply_vault_fallback`'s existing per-endpoint loop already skips any endpoint
    // whose `api_key` is already `Some`. Nothing in `apply_vault_fallback` itself is touched
    // by this plan (see the plan's D-11 machine check) — this test only exercises the
    // EXISTING behavior against a sibling-shaped scenario.
    // =========================================================================

    #[tokio::test]
    async fn sibling_env_var_installed_before_build_wins_over_a_root_vault_entry() {
        use ironhermes_core::config::{Config, ProviderConfig};
        use ironhermes_core::{ModelsCache, ProviderResolver};

        const ENV_VAR: &str = "COMPOSITION_TEST_SIBLING_API_KEY";
        const PROVIDER_NAME: &str = "compositiontestsibling";
        const INSTALLED_VALUE: &str = "sk-installed-from-profile-vault";

        // SAFETY: test-only env var mutation, this crate's own documented env-race trap
        // means this runs correctly only under `--test-threads=1`, matching every other
        // test in this module.
        unsafe {
            std::env::set_var(ENV_VAR, INSTALLED_VALUE);
        }

        let mut config = Config::default();
        config.providers.insert(
            PROVIDER_NAME.to_string(),
            ProviderConfig {
                api_key_env: Some(ENV_VAR.to_string()),
                ..Default::default()
            },
        );

        // `build_with_cache` (unlike `build_with_env_overrides_strict`) reads the REAL
        // process environment at priority 1 — the same constructor the real agent runtime
        // uses once a worker process reaches that point, and the one the module doc's
        // "ProviderResolver::build resolves it at priorities 1-4" claim names.
        let mut resolver = ProviderResolver::build_with_cache(&config, ModelsCache::default())
            .expect("resolver build");

        // A root-vault double carrying a DIFFERENT value for the SAME provider name — if
        // `apply_vault_fallback` ever overwrote an already-set key, THIS value would win,
        // which is exactly the regression this test rules out.
        struct RootVaultDouble;
        #[async_trait::async_trait]
        impl ironhermes_vault::SecretStore for RootVaultDouble {
            async fn get_secret(
                &self,
                _key: &str,
            ) -> anyhow::Result<Option<secrecy::SecretString>> {
                Ok(Some(SecretString::from(
                    "sk-root-vault-must-not-win".to_string(),
                )))
            }
            async fn put_secret(
                &self,
                _key: &str,
                _value: secrecy::SecretString,
            ) -> anyhow::Result<()> {
                unreachable!("this test never writes")
            }
            async fn delete_secret(&self, _key: &str) -> anyhow::Result<()> {
                unreachable!("this test never deletes")
            }
            async fn list_secrets(&self, _prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
                unreachable!("this test never lists")
            }
        }

        resolver
            .apply_vault_fallback(&RootVaultDouble)
            .await
            .expect("apply_vault_fallback must not error");

        let resolved = resolver
            .resolve(PROVIDER_NAME)
            .expect("provider must resolve");
        assert_eq!(
            resolved.api_key.as_deref(),
            Some(INSTALLED_VALUE),
            "a credential installed into the provider's env var (priority 1) must win over \
             a root-vault entry for the same provider name — apply_vault_fallback must skip \
             it, never overwrite it"
        );

        // SAFETY: test-only cleanup.
        unsafe {
            std::env::remove_var(ENV_VAR);
        }
    }

    // =========================================================================
    // Phase 51 Plan 15 (CR-04): the cross-path invariant. Before this plan,
    // `profile_migrate::resolve_provider_env_var_name` (two-tier: explicit
    // override, then a built-in-name fallback) and this file's own resolution
    // (one-tier: explicit override only) answered the same question two
    // different ways — for the very common shape `model.provider: openrouter`
    // with no `providers:` block, migration would scrub a profile this
    // bootstrap could never consume. Both now delegate to the SAME
    // `ironhermes_core::provider_env::provider_api_key_env_name`, so this test
    // asserts equality as the durable invariant rather than trusting the two
    // call sites to stay in sync by convention.
    // =========================================================================

    #[test]
    fn migration_and_worker_resolve_the_same_env_var_name_for_every_case() {
        use ironhermes_core::config::{Config, CustomProviderConfig, ProviderConfig};

        // Case: an explicit providers.<name>.api_key_env override wins.
        let mut explicit_override = Config::default();
        explicit_override.providers.insert(
            "openrouter".to_string(),
            ProviderConfig {
                api_key_env: Some("CUSTOM_ENV_NAME".to_string()),
                ..Default::default()
            },
        );

        // Case: no providers: entry at all — each of the three built-ins.
        let no_providers_block = Config::default();

        // Case: a provider name nobody has heard of.
        let unknown_provider = Config::default();

        // Case: a custom_providers entry, which has no api_key_env field at all.
        let mut custom_no_env = Config::default();
        custom_no_env.custom_providers.push(CustomProviderConfig {
            name: "mycustom".to_string(),
            base_url: "https://example.invalid".to_string(),
            api_key: None,
            api_mode: None,
            default_model: None,
        });

        let cases: &[(&str, &Config, &str)] = &[
            ("explicit override", &explicit_override, "openrouter"),
            ("builtin openrouter, no providers: block", &no_providers_block, "openrouter"),
            ("builtin anthropic, no providers: block", &no_providers_block, "anthropic"),
            ("builtin openai, no providers: block", &no_providers_block, "openai"),
            ("unknown provider", &unknown_provider, "totally-unknown-provider"),
            ("custom provider, no api_key_env field", &custom_no_env, "mycustom"),
        ];

        for (label, config, provider) in cases {
            let migration_answer = crate::profile_migrate::resolve_provider_env_var_name(config, provider);
            let worker_answer = resolve_worker_api_key_env(config, provider);
            assert_eq!(
                migration_answer, worker_answer,
                "migration and worker must resolve the SAME env-var name for case {label:?} \
                 (provider {provider:?}) — got migration={migration_answer:?}, \
                 worker={worker_answer:?}"
            );
        }
    }

    // =========================================================================
    // Code review fixes (CR-01 / WR-01), post-Plan-18 — in-process log-event proof.
    //
    // The two subprocess regression tests in `tests/worker_vault_bootstrap.rs` prove the
    // BEHAVIORAL half of each fix (the installed/resolved value). They cannot ALSO prove the
    // LOGGED half: `main.rs` calls `bootstrap_worker_credential` (main.rs's call site, well
    // before `tracing_subscriber::fmt()...init()` runs) and the `test-oracles` hash-print
    // hook exits the child via `std::process::exit(0)` before that subscriber install is ever
    // reached — so every `tracing::info!`/`tracing::warn!` emitted during THIS function, in
    // every subprocess test in that file, has no installed subscriber to reach. That is a
    // pre-existing architectural fact (predates this fix, and applies to production too —
    // out of this review-fix's scope to change), not something these tests can route around
    // without a real subscriber. These two tests instead call `bootstrap_worker_credential`
    // directly, in-process, against a minimal fake Unix-socket server (mirroring
    // `ironhermes_vault::profile_client`'s own fake-server test pattern), with a
    // `tracing_subscriber::Layer` installed via `tracing::subscriber::set_default` for the
    // duration of the call — proving the named event actually fires, not just that the
    // resulting behavior is correct.
    // =========================================================================

    /// Minimal capturing layer: records every field of every event that reaches it as one
    /// "key1=value1 key2=value2 ..."-shaped string per event, so a test can assert on the
    /// `event`/`provider`/`provider_env_var` fields a `tracing::warn!`/`tracing::info!` call
    /// carried — mirrors `main.rs`'s own `nf1_rusty_vault_log_silence_46_8_gap::CaptureLayer`.
    #[derive(Clone, Default)]
    struct FieldCaptureLayer(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

    struct FieldCaptureVisitor(String);
    impl tracing::field::Visit for FieldCaptureVisitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write;
            let _ = write!(self.0, " {}={:?}", field.name(), value);
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for FieldCaptureLayer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut visitor = FieldCaptureVisitor(String::new());
            event.record(&mut visitor);
            self.0.lock().unwrap().push(visitor.0);
        }
    }

    /// Bind a fake Unix-socket credential endpoint that accepts ONE connection and answers
    /// exactly `responses.len()` requests, matching each response to the request's own `key`
    /// field (order-independent) — a minimal stand-in for the real
    /// `ironhermes_vault::host_profile_credential_endpoint`, mirroring
    /// `ironhermes_vault::profile_client`'s own fake-server test pattern. Returns the socket
    /// path and the server task's `JoinHandle` (callers must `.await` it after the client
    /// call completes).
    fn spawn_fake_credential_endpoint(
        responses: Vec<(&'static str, String)>,
    ) -> (std::path::PathBuf, tokio::task::JoinHandle<()>) {
        let socket_path = std::env::temp_dir().join(format!(
            "ihcli-worker-bootstrap-fake-endpoint-{}-{}.sock",
            std::process::id(),
            responses.len()
        ));
        let _ = std::fs::remove_file(&socket_path);
        let listener =
            tokio::net::UnixListener::bind(&socket_path).expect("bind fake credential endpoint");

        let handle = tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            let (stream, _addr) = listener.accept().await.expect("accept");
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            for _ in 0..responses.len() {
                let mut line = String::new();
                reader
                    .read_line(&mut line)
                    .await
                    .expect("read request line");
                let req: serde_json::Value =
                    serde_json::from_str(line.trim()).expect("request line must be valid JSON");
                let key = req.get("key").and_then(|v| v.as_str()).unwrap_or_default();
                let body = responses
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, body)| body.clone())
                    .unwrap_or_else(|| "{\"error\":\"secret_not_found\"}".to_string());
                writer.write_all(body.as_bytes()).await.unwrap();
                writer.write_all(b"\n").await.unwrap();
                writer.flush().await.unwrap();
            }
        });

        (socket_path, handle)
    }

    /// CR-01: two providers sharing the same `api_key_env`, both resolvable over the fake
    /// socket. The named collision-skip event must fire for the SIBLING, and the MAIN
    /// provider's value must be what ends up installed — never overwritten.
    #[tokio::test]
    async fn sibling_env_var_collision_is_skipped_with_a_named_event_and_main_wins() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.yaml"),
            "model:\n  provider: collidemain\nproviders:\n  openrouter:\n    disabled: true\n  anthropic:\n    disabled: true\n  openai:\n    disabled: true\n  collidemain:\n    api_key_env: SHARED_COLLISION_ENV\n  collidesibling:\n    api_key_env: SHARED_COLLISION_ENV\n",
        )
        .unwrap();

        let main_value = "sk-inproc-collide-main";
        let sibling_value = "sk-inproc-collide-sibling";
        let (socket_path, server) = spawn_fake_credential_endpoint(vec![
            ("collidemain", format!("{{\"value\":\"{main_value}\"}}")),
            ("collidesibling", format!("{{\"value\":\"{sibling_value}\"}}")),
        ]);

        clear_vault_vars();
        // SAFETY: test-only env var mutation, this crate's own documented env-race trap
        // means this runs correctly only under `--test-threads=1`, matching every other
        // test in this module.
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
            std::env::set_var(VAULT_TOKEN_ENV, "bogus-token-never-validated");
            std::env::set_var(VAULT_SOCKET_ENV, &socket_path);
            std::env::remove_var("SHARED_COLLISION_ENV");
        }

        use tracing_subscriber::layer::SubscriberExt as _;
        let captured: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
        let capture_layer = FieldCaptureLayer(captured.clone());
        let subscriber = tracing_subscriber::registry().with(capture_layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        let outcome = bootstrap_worker_credential(Some("collide"))
            .await
            .expect("bootstrap must succeed despite the env-var collision");

        drop(_guard);
        server.await.expect("fake server task must not panic");

        assert!(
            matches!(
                &outcome,
                BootstrapOutcome::Installed { provider_env_var } if provider_env_var == "SHARED_COLLISION_ENV"
            ),
            "expected Installed{{provider_env_var: SHARED_COLLISION_ENV}}, got {outcome:?}"
        );
        assert_eq!(
            std::env::var("SHARED_COLLISION_ENV").as_deref(),
            Ok(main_value),
            "the MAIN provider must win the shared env var — a sibling must never overwrite it"
        );

        let events = captured.lock().unwrap();
        assert!(
            events.iter().any(|e| {
                e.contains("worker_vault_sibling_env_var_collision_skipped")
                    && e.contains("collidesibling")
                    && e.contains("SHARED_COLLISION_ENV")
            }),
            "expected the named collision-skip event naming the sibling and the colliding \
             env var, got: {events:#?}"
        );

        // SAFETY: test-only cleanup.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
            std::env::remove_var("SHARED_COLLISION_ENV");
        }
        clear_vault_vars();
        let _ = std::fs::remove_file(&socket_path);
    }

    /// WR-01: the main provider's read succeeds; a sibling's read fails with `denied` — a
    /// named error that is NEITHER `secret_not_found` NOR `invalid_key`. The bootstrap must
    /// still succeed, the main credential must still install, and the named sibling-read-
    /// failure-skip event must fire naming the sibling and the reason.
    #[tokio::test]
    async fn sibling_read_failure_is_skipped_with_a_named_event_and_main_survives() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.yaml"),
            "model:\n  provider: wr01main\nproviders:\n  openrouter:\n    disabled: true\n  anthropic:\n    disabled: true\n  openai:\n    disabled: true\n  wr01main:\n    api_key_env: WR01MAIN_INPROC_ENV\n  wr01sibling:\n    api_key_env: WR01SIBLING_INPROC_ENV\n",
        )
        .unwrap();

        let main_value = "sk-inproc-wr01-main";
        let (socket_path, server) = spawn_fake_credential_endpoint(vec![
            ("wr01main", format!("{{\"value\":\"{main_value}\"}}")),
            ("wr01sibling", "{\"error\":\"denied\"}".to_string()),
        ]);

        clear_vault_vars();
        // SAFETY: test-only env var mutation, same justification as above.
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
            std::env::set_var(VAULT_TOKEN_ENV, "bogus-token-never-validated");
            std::env::set_var(VAULT_SOCKET_ENV, &socket_path);
            std::env::remove_var("WR01MAIN_INPROC_ENV");
        }

        use tracing_subscriber::layer::SubscriberExt as _;
        let captured: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
        let capture_layer = FieldCaptureLayer(captured.clone());
        let subscriber = tracing_subscriber::registry().with(capture_layer);
        let _guard = tracing::subscriber::set_default(subscriber);

        let outcome = bootstrap_worker_credential(Some("wr01"))
            .await
            .expect("a sibling-only failure must not abort a worker whose main credential \
                     already resolved");

        drop(_guard);
        server.await.expect("fake server task must not panic");

        assert!(
            matches!(
                &outcome,
                BootstrapOutcome::Installed { provider_env_var } if provider_env_var == "WR01MAIN_INPROC_ENV"
            ),
            "expected Installed{{provider_env_var: WR01MAIN_INPROC_ENV}}, got {outcome:?}"
        );
        assert_eq!(
            std::env::var("WR01MAIN_INPROC_ENV").as_deref(),
            Ok(main_value),
            "the main provider's credential must still install"
        );

        let events = captured.lock().unwrap();
        assert!(
            events.iter().any(|e| {
                e.contains("worker_vault_sibling_read_failed_skipped") && e.contains("wr01sibling")
            }),
            "expected the named sibling-read-failure-skip event naming the sibling, got: \
             {events:#?}"
        );

        // SAFETY: test-only cleanup.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
            std::env::remove_var("WR01MAIN_INPROC_ENV");
        }
        clear_vault_vars();
        let _ = std::fs::remove_file(&socket_path);
    }
}

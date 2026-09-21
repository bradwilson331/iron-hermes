//! Hard pre-spawn dispatch gate predicate (Phase 47.4 Plan 10, GAP-1;
//! relocated here by the 47.4 UAT inline fix).
//!
//! UAT proved a kanban worker can be spawned against a profile that cannot
//! actually reach its configured LLM provider — profile `bdev01` (provider
//! `moonshot`, no `MOONSHOT_API_KEY`) was dispatched and died `401
//! Unauthorized` ~1s after spawn. This module is the single, shared,
//! provider-aware "can this profile actually dispatch" predicate that makes
//! that dispatch impossible: [`evaluate_profile_dispatch_at`] resolves the
//! profile's own `config.yaml` + `.env` through the D-14
//! [`crate::provider::ProviderResolver::build_with_env_overrides_strict`]
//! primitive — STRICT, so the ambient process environment never leaks into the
//! answer (see the third-root-cause note below).
//!
//! # Why this lives in `ironhermes-core`
//!
//! Plan 10 originally placed this predicate in `ironhermes-cli` and wired it
//! only into `cmd_dispatch` — the one-shot `ironhermes kanban dispatch`
//! command. The 47.4 UAT then caught a worker spawning and dying anyway:
//! the dispatcher that actually runs in production is
//! `ironhermes_kanban::run_dispatch_loop`, spawned by the **gateway**
//! (`ironhermes-gateway/src/runner.rs`), which calls `run_dispatch_tick` on
//! an interval and never went near the CLI command. The task stayed
//! `status='ready'` and was only ever caught post-hoc by
//! `respawn_guard_reason`'s `blocker_auth` branch — which by construction can
//! only fire *after* a spawn has already failed.
//!
//! `ironhermes-kanban` cannot depend on `ironhermes-cli` (that direction is
//! already taken: cli → kanban), so the predicate lives here, in the crate
//! both of them — and `iron_hermes_ui` — already depend on. Every dispatch
//! path now shares one definition:
//!
//! | Caller | Path |
//! |---|---|
//! | `ironhermes_kanban::dispatcher` | per-task gate inside `run_dispatch_tick_for_board`, before claim/spawn |
//! | `ironhermes_cli::kanban::commands::cmd_dispatch` | pre-tick sweep (re-exported shim) |
//! | `iron_hermes_ui` profile health + VERIFY | `evaluate_profile_dispatch` directly |
//!
//! # Third root cause (47.4 UAT): the gate must use the WORKER's environment
//!
//! The predicate resolves keys with the process-env fallback DISABLED. The
//! permissive `build_with_env_overrides` answers "can *this process* reach the
//! provider?", which is the wrong question here and false-ALLOWs: the gateway
//! loads the ROOT `~/.ironhermes/.env` into its own environment
//! (`ironhermes-cli/src/main.rs`), but the worker it spawns runs under
//! `.env_clear()` with only 7 safe system vars
//! (`ironhermes-kanban/src/worker_spawn.rs`) and sees ONLY the target
//! profile's `.env`. Any profile whose keys are a subset of root's — the
//! normal case — was judged reachable and then died `401` ~1s after spawn.
//!
//! There is no runtime inheritance being broken by this: the wizard's "key
//! inheritance" COPIES keys into the profile's `.env` at creation time.
//!
//! Every branch fails closed (Refuse) — a load error, a parse error, an
//! unknown provider, or a missing key never produces `Allow`. The one
//! deliberate exception is the keyless-provider carve-out (step 6 below):
//! a provider that declares no key source at all (e.g. a local `llama`
//! endpoint with `api_key_env: null` / `api_key: null`) is legitimately
//! dispatchable and must not be refused.
//!
//! # The vault branch (Phase 51 D-14) — a second, still fail-closed path to Allow
//!
//! The strict `.env`-only resolution above (steps 1-9) runs FIRST and stays
//! byte-identical for a profile that has not been migrated to vault-backed
//! credential storage. Only when that resolution refuses for lack of a key does
//! this module try a second path: if `config.vault.enabled` and
//! `config.vault.backend == "rusty-vault"`, it asks
//! [`crate::vault::resolve_vault_config`] to open the real store and lists the
//! profile's own secret names through [`ironhermes_vault::ProfileSecretStore::list_profile_secret_names`]
//! — names only, never a value — via the [`ProfileSecretLister`] seam below. A
//! listing that contains the provider name is [`DispatchDecision::AllowFromVault`];
//! everything else (vault disabled, unreachable, sealed, uninitialized, feature not
//! compiled in, or a listing that does not contain the provider) is a `Refuse`,
//! with the vault-disabled case reusing the exact `.env`-only reason string above.
//!
//! This entry point is therefore async end to end — `ProfileSecretStore`'s methods
//! are `#[async_trait]` — and every production caller must await it. No blocking
//! bridge (`block_in_place`, a nested runtime) is used to keep it synchronous:
//! `iron_hermes_ui` polls its server functions inside a per-connection `LocalSet`,
//! where `block_in_place` panics despite a multi-thread runtime.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::constants::{PROFILES_SUBDIR, get_hermes_home};
use crate::models_cache::ModelsCache;
use crate::provider::ProviderResolver;

// Phase 51 Plan 15 (CR-04): the built-in provider name list this module used
// to keep as its own private `BUILTIN_PROVIDERS` array now lives ONCE, in
// `crate::provider_env::BUILTIN_PROVIDER_ENV_VARS` — both call sites below
// read `crate::provider_env::is_builtin_provider` instead.

/// Stable, greppable marker prefixing every reason string this gate writes
/// into a `block_task` call (T-47.4-10-04).
pub const DISPATCH_GATE_REASON_PREFIX: &str = "dispatch gate: ";

/// Outcome of evaluating whether a profile can actually dispatch against its
/// configured main provider.
///
/// Exactly three variants (Phase 51 D-14): `Allow` stays a bare unit variant with
/// no fields, unchanged from before this phase — an un-migrated profile must be
/// judged byte-identically, and a field on `Allow` would change that value for
/// every profile in the repo. `AllowFromVault` is a THIRD variant rather than a
/// field precisely so every existing `match` on this enum becomes non-exhaustive
/// and must be consciously updated. See [`DispatchDecision::is_allowed`] for call
/// sites that only need to know whether dispatch may proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchDecision {
    Allow,
    /// The profile's own `.env` has no key for its configured provider, but the
    /// vault holds one — proven via a names-only listing, never a value read
    /// (Phase 51 D-14/D-15).
    AllowFromVault,
    Refuse { reason: String },
}

impl DispatchDecision {
    /// True for either allow variant. Call sites that only care whether dispatch
    /// may proceed (rather than which path allowed it) should use this instead of
    /// matching `Allow` alone, which would silently exclude vault-backed profiles.
    pub fn is_allowed(&self) -> bool {
        matches!(self, DispatchDecision::Allow | DispatchDecision::AllowFromVault)
    }
}

/// Injectable probe over a profile-scoped vault view (Phase 51 Task 1 test seam).
///
/// The vault branch's only channel to the vault is this trait's
/// [`ProfileSecretLister::list_profile_secret_names`] — never a value-returning
/// method. Production always goes through the blanket impl on
/// [`ironhermes_vault::ProfileSecretStore`] below (feature `rusty-vault`), which
/// does not override [`ProfileSecretLister::get_profile_secret_as_root`] and so
/// inherits its panicking default — `vault_branch_uses_names_only_listing`
/// substitutes a double that returns canned names and otherwise relies on that
/// same panicking default, proving the "names only" property at runtime rather
/// than by convention (T-51-22).
#[async_trait::async_trait]
pub trait ProfileSecretLister: Send + Sync {
    /// Names only, never values (D-14/D-15).
    async fn list_profile_secret_names(
        &self,
        slug: &str,
    ) -> Result<Vec<String>, ironhermes_vault::VaultError>;

    /// Panics unconditionally by default. The dispatch gate never calls this —
    /// existence is proven via [`ProfileSecretLister::list_profile_secret_names`]
    /// alone. Exists on the trait (rather than omitted) so a future accidental
    /// call from this module's own vault branch fails loudly instead of silently
    /// compiling against a different, value-returning type.
    async fn get_profile_secret_as_root(
        &self,
        slug: &str,
        leaf: &str,
    ) -> Result<Option<secrecy::SecretString>, ironhermes_vault::VaultError> {
        let _ = (slug, leaf);
        panic!(
            "ProfileSecretLister::get_profile_secret_as_root must never be called by the \
             dispatch gate — existence is proven via names-only listing (D-14/D-15)"
        );
    }
}

/// Production implementation: forwards to the real, root-authorized listing method
/// and deliberately does NOT override [`ProfileSecretLister::get_profile_secret_as_root`],
/// even though the underlying [`ironhermes_vault::ProfileSecretStore`] can answer it for
/// other callers (Plans 03/06/07) — inheriting the panicking default is what makes the
/// "the dispatch gate never reads a value" property hold for the REAL vault path too, not
/// only for the test double.
#[cfg(feature = "rusty-vault")]
#[async_trait::async_trait]
impl ProfileSecretLister for ironhermes_vault::ProfileSecretStore {
    async fn list_profile_secret_names(
        &self,
        slug: &str,
    ) -> Result<Vec<String>, ironhermes_vault::VaultError> {
        ironhermes_vault::ProfileSecretStore::list_profile_secret_names(self, slug).await
    }
}

/// Evaluate dispatchability for `profile_name` against the operator's real
/// `$IRONHERMES_HOME/profiles/` directory.
pub async fn evaluate_profile_dispatch(profile_name: &str) -> DispatchDecision {
    let profiles_root = get_hermes_home().join(PROFILES_SUBDIR);
    evaluate_profile_dispatch_at(&profiles_root, profile_name).await
}

/// Sync, `.env`-only convenience wrapper for [`evaluate_profile_dispatch_dotenv_at`]
/// against the operator's real `$IRONHERMES_HOME/profiles/` directory — mirrors
/// [`evaluate_profile_dispatch`]'s own path resolution. For callers that
/// deliberately stay on the `.env`-only core (see that function's doc).
pub fn evaluate_profile_dispatch_dotenv(profile_name: &str) -> DispatchDecision {
    let profiles_root = get_hermes_home().join(PROFILES_SUBDIR);
    evaluate_profile_dispatch_dotenv_at(&profiles_root, profile_name)
}

/// Outcome of the shared, synchronous steps-1-through-9 core
/// ([`evaluate_profile_dispatch_core`]), before either public entry point decides
/// what to do about a missing `.env` key.
enum CoreOutcome {
    Allow,
    /// Every step through resolving the endpoint succeeded except the final key
    /// check — the ONLY outcome the vault branch may act on. Carries the parsed
    /// `Config` (for `config.vault`) and the resolved main provider name.
    RefuseNoKey { config: Box<Config>, main: String },
    /// A fully-formed terminal refusal from any earlier step (invalid name,
    /// missing directory, unparseable config/`.env`, unknown provider, resolver
    /// failure, unresolved endpoint) — never touched by the vault branch.
    Refuse(DispatchDecision),
}

/// Steps 1-9 (T-47.4-10-01 through the final key check), unchanged from before
/// Phase 51 except that the terminal "no key resolves from `.env`" case is
/// reported via [`CoreOutcome::RefuseNoKey`] instead of building the `Refuse`
/// value directly — see [`evaluate_profile_dispatch_dotenv_at`] and
/// [`evaluate_profile_dispatch_at`] for what each public entry point does with
/// that outcome. No `unwrap()`, `expect()`, or `panic!` anywhere in this function
/// (fail-closed, T-47.4-10-03).
fn evaluate_profile_dispatch_core(profiles_root: &Path, profile_name: &str) -> CoreOutcome {
    // Step 1: validate the assignee string BEFORE any path join
    // (T-47.4-10-01) — an assignee that is not a valid profile name never
    // reaches a filesystem path join.
    if let Err(e) = crate::profile::validate_profile_name(profile_name) {
        return CoreOutcome::Refuse(DispatchDecision::Refuse {
            reason: format!(
                "assignee \"{profile_name}\" is not a valid profile name ({e}); dispatch requires a profile directory under profiles/"
            ),
        });
    }

    // Step 2: the profile directory must exist.
    let dir: PathBuf = profiles_root.join(profile_name);
    if !dir.is_dir() {
        return CoreOutcome::Refuse(DispatchDecision::Refuse {
            reason: format!(
                "profile \"{profile_name}\" has no directory at {}",
                dir.display()
            ),
        });
    }

    // Step 3: config.yaml must exist and parse.
    let config_path = dir.join("config.yaml");
    if !config_path.is_file() {
        return CoreOutcome::Refuse(DispatchDecision::Refuse {
            reason: format!("profile \"{profile_name}\" has no config.yaml"),
        });
    }
    let config = match Config::load_from(&config_path) {
        Ok(c) => c,
        Err(e) => {
            return CoreOutcome::Refuse(DispatchDecision::Refuse {
                reason: format!("profile \"{profile_name}\" config.yaml did not parse: {e}"),
            });
        }
    };

    // Step 4: .env is optional — a missing file is an empty map, not an
    // error (a fresh profile legitimately has none). A malformed file fails
    // closed.
    //
    // CR-06 (D-13): the PARSE branch must never interpolate the dotenvy error.
    // An earlier version of this comment claimed the error text carried "never a
    // parsed value" — that was wrong, and the reason it was wrong is the whole
    // bug: `dotenvy::Error::LineParse`'s Display (dotenvy-0.15.7/src/errors.rs:40-44)
    // embeds the ENTIRE raw failing line, so the error IS the parsed value. This
    // reason is both logged and PERSISTED as the task's block reason by
    // run_dispatch_loop (ironhermes-kanban/src/dispatcher.rs:1098-1113), where the
    // board renders it — so a leak here is written to a DB, not merely transient.
    //
    // The OPEN branch keeps its detail: that failure is an Io error (missing file,
    // bad permissions) carrying no line content, and the detail is the diagnostic.
    // Pinned by `refuse_reason_never_leaks_the_env_line_content`.
    let env_path = dir.join(".env");
    let overrides: HashMap<String, String> = if env_path.exists() {
        let iter = match dotenvy::from_path_iter(&env_path) {
            Ok(iter) => iter,
            Err(e) => {
                return CoreOutcome::Refuse(DispatchDecision::Refuse {
                    reason: format!(
                        "profile \"{profile_name}\" .env could not be opened: {e} (path: {})",
                        env_path.display()
                    ),
                });
            }
        };
        let mut map = HashMap::new();
        for item in iter {
            match item {
                Ok((k, v)) => {
                    map.insert(k, v);
                }
                Err(_) => {
                    return CoreOutcome::Refuse(DispatchDecision::Refuse {
                        reason: format!(
                            "profile \"{profile_name}\" .env has a malformed line — \
                             content withheld (D-13); repair the file (path: {})",
                            env_path.display()
                        ),
                    });
                }
            }
        }
        map
    } else {
        HashMap::new()
    };

    // Step 5: the main provider must be set.
    let main = config.model.provider.clone();
    if main.is_empty() {
        return CoreOutcome::Refuse(DispatchDecision::Refuse {
            reason: format!("profile \"{profile_name}\" config.yaml sets no model.provider"),
        });
    }

    // Step 6: keyless-provider carve-out (T-47.4-10-03). A provider entry
    // that exists and declares NO key source at all (both `api_key_env` and
    // `api_key` are `None`, and `main` is not a built-in legacy name) is a
    // legitimately keyless local/self-hosted endpoint (e.g. `llama`) and
    // must not be refused.
    if let Some(provider_cfg) = config.providers.get(main.as_str()) {
        let declares_key_source = provider_cfg.api_key_env.is_some()
            || provider_cfg.api_key.is_some()
            || crate::provider_env::is_builtin_provider(main.as_str());
        if !declares_key_source {
            return CoreOutcome::Allow;
        }
    }

    // Step 7: confirm `main` is resolvable before touching the resolver —
    // it must be a known name (explicit `providers:` entry, a built-in
    // legacy name, or a `custom_providers:` entry).
    let known = config.providers.contains_key(main.as_str())
        || crate::provider_env::is_builtin_provider(main.as_str())
        || config.custom_providers.iter().any(|c| c.name == main);
    if !known {
        return CoreOutcome::Refuse(DispatchDecision::Refuse {
            reason: format!(
                "profile \"{profile_name}\" names unknown provider \"{main}\" — not in providers:, custom_providers:, or the built-in set"
            ),
        });
    }

    // Step 8: build the resolver against THIS profile's own `.env`
    // overrides — the D-14 primitive; never mutate the process environment
    // (unsafe in a multi-threaded process) and never the operator's own
    // process env.
    let resolver =
        match ProviderResolver::build_with_env_overrides_strict(&config, ModelsCache::load(), &overrides) {
            Ok(r) => r,
            Err(e) => {
                return CoreOutcome::Refuse(DispatchDecision::Refuse {
                    reason: format!("profile \"{profile_name}\" provider resolution failed: {e}"),
                });
            }
        };

    // Step 9: resolve the main endpoint through the non-panicking `resolve`
    // accessor — `resolve_for_main()` PANICS when the main provider is
    // absent from the endpoint map (e.g. disabled), which step 7's
    // name-membership check alone cannot rule out. Never call it here.
    let endpoint = match resolver.resolve(&main) {
        Some(ep) => ep,
        None => {
            return CoreOutcome::Refuse(DispatchDecision::Refuse {
                reason: format!(
                    "profile \"{profile_name}\" configured provider \"{main}\" did not resolve to an endpoint (disabled or misconfigured)"
                ),
            });
        }
    };

    let key_present = endpoint
        .api_key
        .as_ref()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);
    if !key_present {
        return CoreOutcome::RefuseNoKey {
            config: Box::new(config),
            main,
        };
    }

    CoreOutcome::Allow
}

/// The `.env`-only strict core, synchronous, with NO vault branch — the seat the
/// existing eleven-case fail-closed regression file
/// (`ironhermes-cli/tests/dispatch_profile_gate.rs`) drives directly, so those
/// assertions stay a real regression rather than a rewrite. Behaves exactly as
/// [`evaluate_profile_dispatch_at`] did before Phase 51: a missing `.env` key is
/// always a `Refuse`, regardless of `config.vault`.
pub fn evaluate_profile_dispatch_dotenv_at(
    profiles_root: &Path,
    profile_name: &str,
) -> DispatchDecision {
    match evaluate_profile_dispatch_core(profiles_root, profile_name) {
        CoreOutcome::Allow => DispatchDecision::Allow,
        CoreOutcome::RefuseNoKey { main, .. } => dotenv_no_key_refuse(profile_name, &main),
        CoreOutcome::Refuse(decision) => decision,
    }
}

/// The vault-aware entry point every production dispatch path calls. Runs the
/// SAME steps 1-9 as [`evaluate_profile_dispatch_dotenv_at`] — that strict
/// `.env`-only resolution stays first and unchanged — and only on its specific
/// "no key resolves from `.env`" outcome does it try the vault branch (D-14). See
/// the module doc's "The vault branch" section.
pub async fn evaluate_profile_dispatch_at(
    profiles_root: &Path,
    profile_name: &str,
) -> DispatchDecision {
    match evaluate_profile_dispatch_core(profiles_root, profile_name) {
        CoreOutcome::Allow => DispatchDecision::Allow,
        CoreOutcome::RefuseNoKey { config, main } => {
            evaluate_vault_or_refuse(profile_name, &main, &config).await
        }
        CoreOutcome::Refuse(decision) => decision,
    }
}

/// The reason returned when neither the `.env`-only resolution above NOR the vault
/// branch below can allow dispatch, and the vault was never consulted at all
/// (disabled, or configured for a non-`rusty-vault` backend) — byte-identical to
/// the reason this module has always returned for this case, so an un-migrated
/// profile's Refuse is unaffected by this phase (D-14 zero-behavioral-change).
fn dotenv_no_key_refuse(profile_name: &str, main: &str) -> DispatchDecision {
    DispatchDecision::Refuse {
        reason: format!(
            "profile \"{profile_name}\" is configured for provider \"{main}\" but no key for that provider resolves from its .env"
        ),
    }
}

/// Reached only once the strict `.env`-only resolution has refused. Falls straight
/// through to [`dotenv_no_key_refuse`], unchanged, when the vault is not enabled or
/// not configured for the `rusty-vault` backend — vault-disabled deployments (the
/// default install) see zero behavior change. Otherwise tries the vault branch.
async fn evaluate_vault_or_refuse(profile_name: &str, main: &str, config: &Config) -> DispatchDecision {
    if !(config.vault.enabled && config.vault.backend == "rusty-vault") {
        return dotenv_no_key_refuse(profile_name, main);
    }
    evaluate_vault_branch(profile_name, main, config).await
}

/// Refuse reason naming vault UNREACHABILITY — sealed, uninitialized, denied, an
/// opaque backend error, or the `rusty-vault` feature not compiled in. Distinct
/// from [`vault_no_secret_refuse`] so an operator can tell "the vault is down, fix
/// the vault" from "this profile was never migrated, run the migration".
fn vault_unreachable_refuse(profile_name: &str, main: &str, detail: &str) -> DispatchDecision {
    DispatchDecision::Refuse {
        reason: format!(
            "profile \"{profile_name}\" is configured for provider \"{main}\" but its \
             vault-backed credential resolution is unreachable ({detail})"
        ),
    }
}

/// Refuse reason naming an unconfigured VAULT-BACKED profile: the vault is
/// reachable and authorized, but its listing for this profile does not contain the
/// configured provider — semantically the same "genuinely unconfigured" case as
/// today's missing-`.env`-key, but pointing the operator at the vault migration
/// instead.
fn vault_no_secret_refuse(profile_name: &str, main: &str) -> DispatchDecision {
    DispatchDecision::Refuse {
        reason: format!(
            "profile \"{profile_name}\" has no key for provider \"{main}\" in its .env, and \
             the vault holds no secret for that provider at secret/profiles/{profile_name}/{main} \
             — run `ironhermes vault migrate-profile {profile_name}` or add the key to its .env"
        ),
    }
}

/// Refuse reason naming a vault secret that exists but has NOWHERE to be
/// installed (Phase 51 Plan 15, T-51-87): the vault holds a secret for this
/// profile+provider, but [`crate::provider_env::provider_api_key_env_name`]
/// resolves no env-var name for it — an `AllowFromVault` here would let the
/// dispatcher mint a token and spawn a worker that can only ever die with
/// `NoApiKeyEnv`. Follows [`vault_no_secret_refuse`]'s shape and, like it,
/// never echoes `.env` content (T-51-25's control) — only the profile and
/// provider names.
fn vault_no_env_var_refuse(profile_name: &str, main: &str) -> DispatchDecision {
    DispatchDecision::Refuse {
        reason: format!(
            "profile \"{profile_name}\" has a vault secret for provider \"{main}\", but no \
             env-var name resolves for it — add providers.{main}.api_key_env to config.yaml \
             (or use one of the built-in provider names) so a spawned worker knows which \
             variable to install the credential into"
        ),
    }
}

/// Open the real vault and delegate to [`evaluate_vault_branch_with_lister`]. Any
/// error opening the store (sealed, uninitialized, opaque backend failure) is
/// mapped to [`vault_unreachable_refuse`] — never a silent fall-back to the
/// `.env`-only answer, matching `apply_vault_fallback`'s existing
/// hard-error-on-sealed-vault posture (D-07).
#[cfg(feature = "rusty-vault")]
async fn evaluate_vault_branch(profile_name: &str, main: &str, config: &Config) -> DispatchDecision {
    let vault_cfg = crate::vault::resolve_vault_config(config);
    // Phase 51 Plan 17 (CR-05 / T-51-67): `open_async`, never the sync `open` — this fn is
    // awaited directly from `decide_spawn_credential` (no `spawn_blocking` wrapper at that
    // call site), so the sync bridge's blocking `rx.recv()` must never run on this task's own
    // thread. See `open_async`'s doc for the full rationale.
    let store = match ironhermes_vault::RustyVaultStore::open_async(&vault_cfg.rusty_vault).await {
        Ok(s) => s,
        Err(e) => return vault_unreachable_refuse(profile_name, main, &e.to_string()),
    };
    let profile_store = ironhermes_vault::ProfileSecretStore::from_rusty_vault_store(&store);
    evaluate_vault_branch_with_lister(profile_name, main, config, &profile_store).await
}

/// `rusty-vault` not compiled in: the vault branch can never be reachable, so this
/// refuses exactly as [`ironhermes_vault::VaultError::BackendUnavailable`] would —
/// still bucketed under "unreachable", per the module doc.
#[cfg(not(feature = "rusty-vault"))]
async fn evaluate_vault_branch(profile_name: &str, main: &str, _config: &Config) -> DispatchDecision {
    vault_unreachable_refuse(
        profile_name,
        main,
        "vault backend unavailable — the `rusty-vault` feature was not compiled in",
    )
}

/// The vault branch's actual decision logic, parameterized over the
/// [`ProfileSecretLister`] seam so `vault_branch_uses_names_only_listing` can drive
/// it against a double instead of a real vault. Never reads a value: the ONLY
/// method called on `lister` is `list_profile_secret_names`.
///
/// Phase 51 Plan 15 (T-51-87): a listing that DOES contain the provider is no
/// longer sufficient for `AllowFromVault` on its own — the provider must also
/// resolve to an env-var name via [`crate::provider_env::provider_api_key_env_name`],
/// or a vault-backed dispatch would be allowed with nowhere to install the
/// credential.
async fn evaluate_vault_branch_with_lister(
    profile_name: &str,
    main: &str,
    config: &Config,
    lister: &dyn ProfileSecretLister,
) -> DispatchDecision {
    match lister.list_profile_secret_names(profile_name).await {
        Ok(names) => {
            if names.iter().any(|n| n == main) {
                if crate::provider_env::provider_api_key_env_name(config, main).is_none() {
                    vault_no_env_var_refuse(profile_name, main)
                } else {
                    DispatchDecision::AllowFromVault
                }
            } else {
                vault_no_secret_refuse(profile_name, main)
            }
        }
        Err(e) => vault_unreachable_refuse(profile_name, main, &e.to_string()),
    }
}

/// Test-only seam (Phase 51 Task 1, `#[doc(hidden)]`): drives the vault branch
/// directly against an injected [`ProfileSecretLister`], skipping real vault I/O
/// entirely. Exists so `vault_branch_uses_names_only_listing` (an external
/// integration test — it cannot see `pub(crate)`/private items) can prove the
/// branch never calls a value-returning method. Not part of the production
/// contract; production always goes through [`evaluate_profile_dispatch_at`].
#[doc(hidden)]
pub async fn evaluate_vault_branch_for_test(
    profile_name: &str,
    main: &str,
    config: &Config,
    lister: &dyn ProfileSecretLister,
) -> DispatchDecision {
    evaluate_vault_branch_with_lister(profile_name, main, config, lister).await
}

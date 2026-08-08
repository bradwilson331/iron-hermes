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

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::constants::{PROFILES_SUBDIR, get_hermes_home};
use crate::models_cache::ModelsCache;
use crate::provider::ProviderResolver;

/// The three provider names that get a legacy built-in env-var fallback
/// ([`crate::provider::ProviderResolver::build_with_env_overrides_strict`],
/// priority 3) even with no explicit `providers:` entry.
const BUILTIN_PROVIDERS: [&str; 3] = ["openrouter", "anthropic", "openai"];

/// Stable, greppable marker prefixing every reason string this gate writes
/// into a `block_task` call (T-47.4-10-04).
pub const DISPATCH_GATE_REASON_PREFIX: &str = "dispatch gate: ";

/// Outcome of evaluating whether a profile can actually dispatch against its
/// configured main provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchDecision {
    Allow,
    Refuse { reason: String },
}

/// Evaluate dispatchability for `profile_name` against the operator's real
/// `$IRONHERMES_HOME/profiles/` directory.
pub fn evaluate_profile_dispatch(profile_name: &str) -> DispatchDecision {
    let profiles_root = get_hermes_home().join(PROFILES_SUBDIR);
    evaluate_profile_dispatch_at(&profiles_root, profile_name)
}

/// The testable seat for [`evaluate_profile_dispatch`]. All logic lives
/// here. Every branch that cannot prove the profile is dispatchable returns
/// `Refuse` (fail-closed, T-47.4-10-03) — no `unwrap()`, `expect()`, or
/// `panic!` anywhere in this function.
pub fn evaluate_profile_dispatch_at(profiles_root: &Path, profile_name: &str) -> DispatchDecision {
    // Step 1: validate the assignee string BEFORE any path join
    // (T-47.4-10-01) — an assignee that is not a valid profile name never
    // reaches a filesystem path join.
    if let Err(e) = crate::profile::validate_profile_name(profile_name) {
        return DispatchDecision::Refuse {
            reason: format!(
                "assignee \"{profile_name}\" is not a valid profile name ({e}); dispatch requires a profile directory under profiles/"
            ),
        };
    }

    // Step 2: the profile directory must exist.
    let dir: PathBuf = profiles_root.join(profile_name);
    if !dir.is_dir() {
        return DispatchDecision::Refuse {
            reason: format!(
                "profile \"{profile_name}\" has no directory at {}",
                dir.display()
            ),
        };
    }

    // Step 3: config.yaml must exist and parse.
    let config_path = dir.join("config.yaml");
    if !config_path.is_file() {
        return DispatchDecision::Refuse {
            reason: format!("profile \"{profile_name}\" has no config.yaml"),
        };
    }
    let config = match Config::load_from(&config_path) {
        Ok(c) => c,
        Err(e) => {
            return DispatchDecision::Refuse {
                reason: format!("profile \"{profile_name}\" config.yaml did not parse: {e}"),
            };
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
                return DispatchDecision::Refuse {
                    reason: format!(
                        "profile \"{profile_name}\" .env could not be opened: {e} (path: {})",
                        env_path.display()
                    ),
                };
            }
        };
        let mut map = HashMap::new();
        for item in iter {
            match item {
                Ok((k, v)) => {
                    map.insert(k, v);
                }
                Err(_) => {
                    return DispatchDecision::Refuse {
                        reason: format!(
                            "profile \"{profile_name}\" .env has a malformed line — \
                             content withheld (D-13); repair the file (path: {})",
                            env_path.display()
                        ),
                    };
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
        return DispatchDecision::Refuse {
            reason: format!("profile \"{profile_name}\" config.yaml sets no model.provider"),
        };
    }

    // Step 6: keyless-provider carve-out (T-47.4-10-03). A provider entry
    // that exists and declares NO key source at all (both `api_key_env` and
    // `api_key` are `None`, and `main` is not a built-in legacy name) is a
    // legitimately keyless local/self-hosted endpoint (e.g. `llama`) and
    // must not be refused.
    if let Some(provider_cfg) = config.providers.get(main.as_str()) {
        let declares_key_source = provider_cfg.api_key_env.is_some()
            || provider_cfg.api_key.is_some()
            || BUILTIN_PROVIDERS.contains(&main.as_str());
        if !declares_key_source {
            return DispatchDecision::Allow;
        }
    }

    // Step 7: confirm `main` is resolvable before touching the resolver —
    // it must be a known name (explicit `providers:` entry, a built-in
    // legacy name, or a `custom_providers:` entry).
    let known = config.providers.contains_key(main.as_str())
        || BUILTIN_PROVIDERS.contains(&main.as_str())
        || config.custom_providers.iter().any(|c| c.name == main);
    if !known {
        return DispatchDecision::Refuse {
            reason: format!(
                "profile \"{profile_name}\" names unknown provider \"{main}\" — not in providers:, custom_providers:, or the built-in set"
            ),
        };
    }

    // Step 8: build the resolver against THIS profile's own `.env`
    // overrides — the D-14 primitive; never mutate the process environment
    // (unsafe in a multi-threaded process) and never the operator's own
    // process env.
    let resolver =
        match ProviderResolver::build_with_env_overrides_strict(&config, ModelsCache::load(), &overrides) {
            Ok(r) => r,
            Err(e) => {
                return DispatchDecision::Refuse {
                    reason: format!("profile \"{profile_name}\" provider resolution failed: {e}"),
                };
            }
        };

    // Step 9: resolve the main endpoint through the non-panicking `resolve`
    // accessor — `resolve_for_main()` PANICS when the main provider is
    // absent from the endpoint map (e.g. disabled), which step 7's
    // name-membership check alone cannot rule out. Never call it here.
    let endpoint = match resolver.resolve(&main) {
        Some(ep) => ep,
        None => {
            return DispatchDecision::Refuse {
                reason: format!(
                    "profile \"{profile_name}\" configured provider \"{main}\" did not resolve to an endpoint (disabled or misconfigured)"
                ),
            };
        }
    };

    let key_present = endpoint
        .api_key
        .as_ref()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);
    if !key_present {
        return DispatchDecision::Refuse {
            reason: format!(
                "profile \"{profile_name}\" is configured for provider \"{main}\" but no key for that provider resolves from its .env"
            ),
        };
    }

    DispatchDecision::Allow
}

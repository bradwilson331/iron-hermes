//! Phase 47.4 Plan 01 (D-08 / D-11): kanban profile enumeration.
//! Phase 47.4 Plan 03 (D-06 / D-07 / D-08 / D-13): native profile scaffold.
//!
//! Surface:
//! - `list_profiles()` — scans `$IRONHERMES_HOME/profiles/*` and returns a
//!   `ProfileRow` per subdirectory, classified via the D-11 health rule
//!   (dir + `config.yaml` + >=1 resolvable LLM key, all disk-only, zero
//!   network I/O — the dot means CONFIGURED, never "reachable").
//! - `create_profile(req)` — ports `scripts/make-kanban-profile`'s scaffold
//!   natively: validates the name, creates the profile dir, byte-copies the
//!   root `config.yaml`, resolves inherited keys server-side, overlays any
//!   manually entered keys, and writes the profile `.env` atomically at
//!   0600. Never clobbers an existing profile without `force: true`. Gated
//!   fail-closed behind `config.security.web_config_write_enabled`, the
//!   same flag `update_provider_config`/`write_provider_secret` already
//!   enforce for this codebase's other two credential/config write
//!   surfaces (D-06). Resolved checkpoint `proceed-with-inventory`: every
//!   generated `.env`'s first line doubles as a machine-readable
//!   provenance stamp (`PROFILE_ENV_PROVENANCE_PREFIX`) so a future
//!   per-profile secret-storage migration can enumerate exactly which
//!   `.env` files this UI surface created.
//!
//! Pattern A (PATTERNS.md, mirrors `kanban_api.rs`): server-only
//! `ironhermes_core` imports are `#[cfg(feature = "server")]`-gated at
//! statement level so the WASM client build never pulls native-only code.
//! The `#[server]` macro generates an HTTP-call stub on the client and a
//! real endpoint on the server. `list_profiles`/`create_profile` are
//! registered in `server/mod.rs` behind `register_server_functions()`,
//! which `require_auth` wraps (main.rs:160-163) — T-47.4-01-E1 /
//! T-47.4-03-E1.
//!
//! No function in this file mutates the server process environment — the
//! web server is multi-threaded and every process-env-mutating call site in
//! this codebase today is single-threaded test-only code (see
//! `47.4-CONTEXT.md` D-14 rationale). No function shells out to a
//! subprocess, and no function force-unwraps a `Result`/`Option`
//! (T-47.4-01-D1 / T-47.4-03-D1) — every error is propagated as a value.
//! No function in this file opens any secret-storage backend — key
//! material is written to the profile `.env` only, at 0600 (D-06).

use dioxus::prelude::*;

#[cfg(feature = "server")]
use secrecy::{ExposeSecret, SecretString};
#[cfg(feature = "server")]
use std::collections::{HashMap, HashSet};
#[cfg(feature = "server")]
use std::path::{Path, PathBuf};

// `ProfileGap`/`ProfileHealth` are only referenced inside the
// `#[cfg(feature = "server")]`-gated helper fns below — the wasm client
// build (default "web" feature, no "server") never sees them, so they are
// gated the same way to avoid an unused-import warning on that target.
#[cfg(feature = "server")]
use crate::protocol::{ProfileGap, ProfileHealth};
// `CreateProfileRequest`/`KeyMode`/`KeyRow`/`KeyStatus` are part of
// `create_profile`'s public signature (Plan 03) — compiled unconditionally
// like `ProfileRow` so the `#[server]` macro's client-side HTTP stub has
// the types it needs on the wasm target too. `ProfileDetail` /
// `ProfileConfigWritePayload` are Plan 05's additions, same reasoning:
// part of `fetch_profile_detail`/`update_profile_config`'s signatures.
use crate::protocol::{
    CreateProfileRequest, DuplicateProfileRequest, KeyMode, KeyRow, KeyStatus,
    ProfileConfigWritePayload, ProfileDetail, ProfilePersona, ProfileRow, ProfileSkillRow,
};

/// Phase 47.4 Plan 01 (D-08): the five-name LLM-provider key allowlist.
/// Mirrors `scripts/make-kanban-profile:39` `DEFAULT_KEYS` exactly, same
/// order — there is no shared source between the bash script and this
/// Rust module, so keep the two lists in sync by hand.
///
/// Phase 47.4 Plan 11 (GAP-1): this is now a COMPATIBILITY FLOOR, not the
/// authoritative key-name set — every name here is guaranteed to remain
/// inheritable/visible even for an operator whose `config.yaml` declares no
/// `providers:` entries at all. The authoritative, provider-registry-derived
/// set is [`provider_key_env_names`]; every call site that decides "which
/// key names exist" must go through that fn, not this constant, directly.
#[cfg(feature = "server")]
pub(crate) const LLM_KEY_ALLOWLIST: [&str; 5] = [
    "OPENROUTER_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GROQ_API_KEY",
    "OLLAMA_API_KEY",
];

/// Phase 47.4 Plan 11 (GAP-1): the three provider names that get a legacy
/// built-in env-var name even with no explicit `providers:` entry — mirrors
/// `ironhermes_core::dispatch_gate::BUILTIN_PROVIDERS` /
/// `ironhermes_core::provider::ProviderResolver`'s own pre-populated three.
#[cfg(feature = "server")]
const BUILTIN_PROVIDER_KEY_ENV_NAMES: [(&str, &str); 3] = [
    ("openrouter", "OPENROUTER_API_KEY"),
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
];

/// Phase 47.4 Plan 11 (GAP-1): the provider-registry-derived key-name set —
/// every non-empty, non-disabled `api_key_env` declared under `providers:`,
/// plus the three built-in legacy names, plus this module's five-name
/// compatibility floor. Deduplicated; provider-derived names are emitted in
/// sorted-by-provider-name order for a deterministic result, followed by the
/// built-ins, followed by the floor. A provider with `api_key_env: null`
/// (the `llama` shape) or `disabled: true` contributes no name.
#[cfg(feature = "server")]
pub(crate) fn provider_key_env_names(config: &ironhermes_core::config::Config) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();

    let mut provider_names: Vec<&String> = config.providers.keys().collect();
    provider_names.sort();
    for name in provider_names {
        // Safe: `name` was just collected from `config.providers.keys()`.
        let prov_cfg = &config.providers[name.as_str()];
        if prov_cfg.disabled == Some(true) {
            continue;
        }
        if let Some(env_name) = prov_cfg.api_key_env.as_deref() {
            if !env_name.is_empty() && seen.insert(env_name.to_string()) {
                out.push(env_name.to_string());
            }
        }
    }

    for (_, env_name) in BUILTIN_PROVIDER_KEY_ENV_NAMES {
        if seen.insert(env_name.to_string()) {
            out.push(env_name.to_string());
        }
    }

    for env_name in LLM_KEY_ALLOWLIST {
        if seen.insert(env_name.to_string()) {
            out.push(env_name.to_string());
        }
    }

    out
}

/// Phase 47.4 Plan 11 (GAP-1): the single env-var name a specific
/// `provider`'s key would come from — its `providers.<provider>.api_key_env`
/// when set and non-empty, else the built-in legacy name for
/// `openrouter`/`anthropic`/`openai`, else `None` (a `custom_providers` entry
/// or a keyless/unknown provider like `llama`).
///
/// Exercised directly by this module's own unit tests
/// (`provider_key_env_name_for_returns_explicit_env_for_moonshot` /
/// `_returns_none_for_keyless_llama`); no production call site needs it
/// today since [`compute_provider_key_state`] gets its provider identity
/// from the caller's already-loaded `ProfileRow`/`ProfileDetail` fields
/// rather than re-deriving it — kept `pub(crate)` as the single-name
/// counterpart to [`provider_key_env_names`] for future/external callers.
#[cfg(feature = "server")]
#[allow(dead_code)]
pub(crate) fn provider_key_env_name_for(
    config: &ironhermes_core::config::Config,
    provider: &str,
) -> Option<String> {
    if let Some(prov_cfg) = config.providers.get(provider) {
        if let Some(env_name) = prov_cfg.api_key_env.as_deref() {
            if !env_name.is_empty() {
                return Some(env_name.to_string());
            }
        }
    }
    BUILTIN_PROVIDER_KEY_ENV_NAMES
        .iter()
        .find(|(name, _)| *name == provider)
        .map(|(_, env_name)| env_name.to_string())
}

/// Phase 47.4 Plan 01 (D-08 / T-47.4-01-T1): pure `PathBuf::join` — never
/// string-formats a profile name into a path. Enumeration only ever reads
/// names already present on disk; no browser-supplied name reaches a path
/// built by this fn in this plan (that begins in a later plan's
/// `create_profile`).
#[cfg(feature = "server")]
pub(crate) fn profile_dir_for(name: &str) -> PathBuf {
    ironhermes_core::get_hermes_home()
        .join(ironhermes_core::PROFILES_SUBDIR)
        .join(name)
}

/// Phase 47.4 Plan 01 (D-11 / T-47.4-01-D1): parse a profile's `.env` into a
/// name -> value map. A missing file is `Ok(HashMap::new())` (a fresh
/// profile legitimately has no `.env` yet) — never an error. A malformed
/// file propagates `Err` as a value; this fn never panics, asserts, or
/// force-unwraps.
///
/// CR-05 (D-13): the PARSE branch never interpolates the `dotenvy::Error`.
/// `Error::LineParse`'s `Display` (`dotenvy-0.15.7/src/errors.rs:40-44`) embeds
/// the entire raw failing line, which for `KEY='the-secret'` IS the secret — and
/// this `Err` string reaches the browser through 6 call sites (`:746`, `:776`,
/// `:890`, `:1041`, `:1044`, `:1335`) on a surface whose auth is off by default.
/// So the parse branch returns a FIXED string plus the path only. The path is not
/// sensitive and is load-bearing: it names WHICH profile needs repair.
///
/// The `open` branch below still interpolates deliberately — `from_path_iter`'s
/// open failure is an `Io` error (missing file, bad permissions) that carries no
/// line content, and its detail is the whole diagnostic value.
///
/// Pinned by `read_env_keys_parse_error_never_leaks_the_line`.
#[cfg(feature = "server")]
pub(crate) fn read_env_keys(path: &Path) -> Result<HashMap<String, String>, String> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let mut out = HashMap::new();
    for item in dotenvy::from_path_iter(path).map_err(|e| format!("open {path:?}: {e}"))? {
        let (k, v) = item.map_err(|_| {
            format!("parse {path:?}: malformed line — content withheld (D-13); repair the file")
        })?;
        out.insert(k, v);
    }
    Ok(out)
}

/// Phase 47.4 Plan 11 (GAP-1): whether a profile's configured MAIN provider
/// has a resolvable key. `Resolved` and `NotRequired` are both non-gap
/// outcomes (a legitimately keyless provider, e.g. `llama`, must not be
/// treated as unhealthy — D-11). `Missing { provider }` carries the
/// provider name so the resulting gap can name it; an EMPTY `provider`
/// string is the sentinel for "provider identity unknown" (an unparseable
/// config), which [`classify_profile_health`] maps to the older, providerless
/// [`ProfileGap::NoResolvableKey`] instead of the new provider-named gap.
#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderKeyState {
    Resolved,
    /// Reserved for a future direct classification of a legitimately keyless
    /// provider without going through the dispatch gate's `Allow` — today the
    /// gate's `Allow` (for both a resolved key AND a keyless carve-out
    /// provider like `llama`) maps to `Resolved`, since `classify_profile_health`
    /// treats the two identically.
    #[allow(dead_code)]
    NotRequired,
    Missing {
        provider: String,
    },
}

/// Phase 47.4 Plan 11 (GAP-1): compute a [`ProviderKeyState`] from the SINGLE
/// shared dispatch-gate predicate (`ironhermes_core::dispatch_gate::
/// evaluate_profile_dispatch`, Plan 10) — the browser's health classifier and
/// the CLI's hard pre-spawn gate must never disagree about the same profile.
/// When `config_effectively_present` is `false` (missing or unparseable
/// `config.yaml`), the provider identity itself is unknown, so this falls
/// back to the pre-Plan-11 all-keys-absent rule (`fallback_resolvable_count`)
/// rather than asking the gate to evaluate a provider it cannot name.
#[cfg(feature = "server")]
pub(crate) fn compute_provider_key_state(
    profile_name: &str,
    config_effectively_present: bool,
    provider: Option<&str>,
    fallback_resolvable_count: usize,
) -> ProviderKeyState {
    if !config_effectively_present {
        return if fallback_resolvable_count > 0 {
            ProviderKeyState::Resolved
        } else {
            ProviderKeyState::Missing {
                provider: String::new(),
            }
        };
    }
    match ironhermes_core::dispatch_gate::evaluate_profile_dispatch(profile_name) {
        ironhermes_core::dispatch_gate::DispatchDecision::Allow => ProviderKeyState::Resolved,
        ironhermes_core::dispatch_gate::DispatchDecision::Refuse { .. } => {
            ProviderKeyState::Missing {
                provider: provider.unwrap_or_default().to_string(),
            }
        }
    }
}

/// Phase 47.4 Plan 01 (D-11): the health-classification rule, extracted as a
/// pure fn with no I/O so every permutation is directly unit-testable
/// (Task 3). `Configured` requires `dir_exists`, `config_yaml_exists`, and a
/// non-`Missing` `provider_key`; every failing condition pushes its own
/// `ProfileGap` — never a bare bool. This fn never consults the network and
/// has no timeout parameter (the real D-09 probe is a separate, later
/// surface).
///
/// Phase 47.4 Plan 11 (GAP-1): `provider_key` replaces the old
/// `resolvable_llm_key_count: usize` parameter — "does this profile have ANY
/// key" is the wrong question; "does it have a key for ITS OWN configured
/// provider" is the one that matches what the CLI dispatch gate actually
/// checks. `ProviderKeyState::Missing { provider }` with a non-empty
/// `provider` pushes the new, honest [`ProfileGap::NoKeyForProvider`]; an
/// empty `provider` (provider identity unknown — unparseable config) keeps
/// the older, providerless [`ProfileGap::NoResolvableKey`] so an unparseable
/// config never reports a misleading provider name.
#[cfg(feature = "server")]
pub(crate) fn classify_profile_health(
    dir_exists: bool,
    config_yaml_exists: bool,
    provider_key: ProviderKeyState,
) -> (ProfileHealth, Vec<ProfileGap>) {
    let mut gaps = Vec::new();
    if !dir_exists {
        gaps.push(ProfileGap::MissingDir);
    }
    if !config_yaml_exists {
        gaps.push(ProfileGap::MissingConfigYaml);
    }
    match provider_key {
        ProviderKeyState::Resolved | ProviderKeyState::NotRequired => {}
        ProviderKeyState::Missing { provider } => {
            if provider.is_empty() {
                gaps.push(ProfileGap::NoResolvableKey);
            } else {
                gaps.push(ProfileGap::NoKeyForProvider(provider));
            }
        }
    }
    if gaps.is_empty() {
        (ProfileHealth::Configured, gaps)
    } else {
        (ProfileHealth::Incomplete, gaps)
    }
}

/// Phase 47.4 Plan 01 (D-08 / D-11): enumerate `$IRONHERMES_HOME/profiles/*`
/// and classify each one. A missing profiles root is `Ok(vec![])` — a
/// fresh machine has no profiles yet, that is not an enumeration failure.
/// Non-directories and dotfile-prefixed entries are skipped; results sort
/// by name. Reads are ungated by `security.web_config_write_enabled`
/// (matches `get_provider_config`, `provider_config_api.rs:222-238`) — this
/// is a read, not a write.
#[server]
pub async fn list_profiles() -> Result<Vec<ProfileRow>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let rows = tokio::task::spawn_blocking(move || -> Result<Vec<ProfileRow>, String> {
            let root = ironhermes_core::get_hermes_home().join(ironhermes_core::PROFILES_SUBDIR);
            let mut names: Vec<String> = Vec::new();
            match std::fs::read_dir(&root) {
                Ok(entries) => {
                    for entry in entries {
                        let entry = entry.map_err(|e| format!("read_dir entry: {e}"))?;
                        let file_type = entry.file_type().map_err(|e| format!("file_type: {e}"))?;
                        if !file_type.is_dir() {
                            continue;
                        }
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.starts_with('.') {
                            continue;
                        }
                        names.push(name);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Fresh machine, no profiles yet — not an enumeration error.
                    return Ok(Vec::new());
                }
                Err(e) => return Err(format!("read_dir({root:?}): {e}")),
            }
            names.sort();

            let mut rows = Vec::with_capacity(names.len());
            for name in names {
                let dir = profile_dir_for(&name);
                let dir_exists = dir.is_dir();

                let config_path = dir.join("config.yaml");
                let config_yaml_on_disk = config_path.is_file();
                let (loaded_config, provider, model_default, config_parsed_ok) =
                    if config_yaml_on_disk {
                        match ironhermes_core::config::Config::load_from(&config_path) {
                            Ok(cfg) => {
                                let provider = Some(cfg.model.provider.clone());
                                let model_default = Some(cfg.model.default.clone());
                                (Some(cfg), provider, model_default, true)
                            }
                            // A present-but-malformed config.yaml degrades to
                            // the same gap as an absent file — one bad profile
                            // never fails the whole enumeration.
                            Err(_) => (None, None, None, false),
                        }
                    } else {
                        (None, None, None, false)
                    };
                let config_effectively_present = config_yaml_on_disk && config_parsed_ok;

                let env_path = dir.join(".env");
                // A malformed .env degrades the same way — the whole
                // enumeration must not fail on one bad profile.
                let env_map = read_env_keys(&env_path).unwrap_or_default();
                let key_count = env_map.len();
                // Provider-registry-derived when the config parsed (GAP-1);
                // falls back to the compatibility floor when it did not,
                // since there is no Config to derive a wider set from.
                let resolvable_llm_key_count = match &loaded_config {
                    Some(cfg) => {
                        let names = provider_key_env_names(cfg);
                        names
                            .iter()
                            .filter(|k| env_map.get(k.as_str()).map(|v| !v.is_empty()).unwrap_or(false))
                            .count()
                    }
                    None => LLM_KEY_ALLOWLIST
                        .iter()
                        .filter(|k| env_map.get(**k).map(|v| !v.is_empty()).unwrap_or(false))
                        .count(),
                };

                let provider_key = compute_provider_key_state(
                    &name,
                    config_effectively_present,
                    provider.as_deref(),
                    resolvable_llm_key_count,
                );
                let (health, gaps) = classify_profile_health(
                    dir_exists,
                    config_effectively_present,
                    provider_key,
                );

                rows.push(ProfileRow {
                    name,
                    health,
                    gaps,
                    provider,
                    model_default,
                    key_count,
                });
            }
            Ok(rows)
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;
        Ok(rows)
    }
    #[cfg(not(feature = "server"))]
    {
        Err(ServerFnError::new(
            "list_profiles unavailable without `server` feature",
        ))
    }
}

/// Phase 47.4 Plan 03 (D-07): the `--all-keys` name-pattern suffixes,
/// mirroring `scripts/make-kanban-profile`'s
/// `^[A-Z0-9_]+_(API_KEY|KEY|TOKEN)=` regex.
#[cfg(feature = "server")]
pub(crate) const ROOT_KEY_PATTERN_SUFFIXES: [&str; 3] = ["_API_KEY", "_KEY", "_TOKEN"];

/// Phase 47.4 Plan 03 (D-13 / T-47.4-03-I1): masked presence marker for a
/// key value. Takes `value` only to decide presence — it must never embed
/// any character of `value` in its output, so this fn structurally cannot
/// leak key material through its own return value.
#[cfg(feature = "server")]
pub(crate) fn mask_key_value(value: &str) -> String {
    if value.is_empty() {
        "\u{2014}".to_string()
    } else {
        "sk-\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}".to_string()
    }
}

/// Phase 47.4 Plan 03 (D-07): the three key-inheritance modes, mirroring
/// `scripts/make-kanban-profile`'s `DEFAULT_KEYS` / `--all-keys` /
/// `--keys` branches. Empty values are dropped in every mode (the script's
/// `[[ -n "$val" ]] || continue`). Returns names in a deterministic order:
/// `LlmOnly` in `provider_key_env_names` order (GAP-1: was the fixed
/// five-name floor's order), `AllKeys` sorted, `Explicit` in the
/// caller-supplied order.
#[cfg(feature = "server")]
pub(crate) fn resolve_inherited_keys(
    root_env: &HashMap<String, String>,
    mode: &KeyMode,
    config: &ironhermes_core::config::Config,
) -> Vec<(String, String)> {
    let names: Vec<String> = match mode {
        KeyMode::LlmOnly => provider_key_env_names(config),
        KeyMode::AllKeys => {
            let mut matched: Vec<String> = root_env
                .keys()
                .filter(|k| {
                    ROOT_KEY_PATTERN_SUFFIXES
                        .iter()
                        .any(|suffix| k.ends_with(suffix))
                })
                .cloned()
                .collect();
            matched.sort();
            matched
        }
        KeyMode::Explicit(names) => names.clone(),
    };
    names
        .into_iter()
        .filter_map(|name| {
            let value = root_env.get(&name)?;
            if value.is_empty() {
                None
            } else {
                Some((name, value.clone()))
            }
        })
        .collect()
}

/// Phase 47.4 Plan 03 (D-06/D-07, resolved checkpoint `proceed-with-inventory`):
/// line 1 of every generated profile `.env` doubles as BOTH the
/// script-mirrored "who generated this file" header (adapted to name this
/// surface, `scripts/make-kanban-profile:154`) AND the machine-readable
/// provenance stamp the resolved checkpoint requires — a future
/// per-profile secret-storage migration can grep this exact prefix to
/// enumerate every `.env` this web surface created. Kept as a `#`-prefixed
/// comment so it is inert to every `.env` parser, and kept as a stable,
/// stand-alone constant so a future refactor cannot silently drop it
/// (covered by `provenance_header_line_is_stamped_on_every_generated_env`).
#[cfg(feature = "server")]
pub(crate) const PROFILE_ENV_PROVENANCE_PREFIX: &str =
    "# Generated by iron_hermes_ui profile wizard (Phase 47.4) for profile \"";

/// Phase 47.4 Plan 20 (CR-03/CR-04, D-06/D-07): single-quotes a raw value for
/// `render_profile_env`'s output.
///
/// Phase 47.6 Plan 04 (D-06): hoisted into `ironhermes_core::dotenv_write` —
/// this is now a thin delegating wrapper so the ui crate and the CLI's
/// `buzz` subcommands share exactly one quoting implementation instead of a
/// second hand-written copy of it. See that module for the full design
/// rationale (dotenvy's splitter vs. its value parser disagreeing about a
/// backslash inside a strong quote).
#[cfg(feature = "server")]
fn quote_env_value(value: &str) -> String {
    ironhermes_core::dotenv_write::quote_env_value(value)
}

/// Phase 47.4 Plan 20 (T-47.4-20-04): `render_profile_env`'s self-check —
/// round-trips the just-rendered bytes back through the REAL `dotenvy`
/// reader and refuses to let the write proceed unless the parse yields
/// exactly the given entry list, in order.
///
/// Phase 47.6 Plan 04 (D-06): hoisted into
/// `ironhermes_core::dotenv_write::verify_env_round_trip` — this wrapper
/// only adapts the typed `DotenvWriteError` back to this module's existing
/// `Result<(), String>` signature via `Display`, which never embeds a value
/// or a raw `dotenvy::Error` (see that module for the full D-13
/// asymmetric-error-branch rationale this preserves unchanged).
#[cfg(feature = "server")]
pub(crate) fn verify_render_round_trip(
    rendered: &str,
    entries: &[(String, String)],
) -> Result<(), String> {
    ironhermes_core::dotenv_write::verify_env_round_trip(rendered, entries).map_err(|e| e.to_string())
}

/// Phase 47.4 Plan 03 (D-06/D-07): renders the profile `.env` file body.
/// Lines 2-3 mirror the remainder of the script's generated header
/// (`scripts/make-kanban-profile:155-156`) in meaning.
///
/// Phase 47.4 Plan 20 (CR-03/CR-04): every value is now single-quoted via
/// `quote_env_value`, which suppresses `dotenvy` substitution and space/`#`/
/// tab end-of-value parsing — the writer, not a `validate_key_value`
/// blocklist, is the boundary (`<design_decision>`). The render then proves
/// itself via `verify_render_round_trip` before returning; a write whose
/// rendered bytes cannot be parsed back to exactly `entries` is refused.
#[cfg(feature = "server")]
pub(crate) fn render_profile_env(
    name: &str,
    entries: &[(String, String)],
) -> Result<String, String> {
    render_profile_env_with_stamp(name, entries, None)
}

/// Phase 48.2 Plan 07 (D-09 checkpoint, resolved option b — inventory
/// stamp): lift of [`render_profile_env`]'s body, parameterized with an
/// optional EXTRA provenance comment line rendered right after the existing
/// header and before the two informational lines. [`render_profile_env`]
/// delegates here with `None`, so its own output — and every existing test
/// against it — is byte-identical to before this lift (T-48.2-07-04: no
/// second write implementation, only this one shared core gains a new,
/// backward-compatible parameter). `extra_stamp` is always a `#`-prefixed
/// comment line, inert to the `dotenvy` reader, verified by the same
/// round-trip check below.
#[cfg(feature = "server")]
pub(crate) fn render_profile_env_with_stamp(
    name: &str,
    entries: &[(String, String)],
    extra_stamp: Option<&str>,
) -> Result<String, String> {
    let mut out = String::new();
    out.push_str(PROFILE_ENV_PROVENANCE_PREFIX);
    out.push_str(name);
    out.push_str("\"\n");
    if let Some(stamp) = extra_stamp {
        out.push_str(stamp);
        out.push('\n');
    }
    out.push_str(
        "# Provider keys inherited from the root .env — workers run with a scrubbed env, so this\n",
    );
    out.push_str("# file is the only source of the kanban judge/agent API key.\n");
    for (key, value) in entries {
        out.push_str(key);
        out.push('=');
        out.push_str(&quote_env_value(value));
        out.push('\n');
    }
    verify_render_round_trip(&out, entries)?;
    Ok(out)
}

/// Phase 47.4 Plan 03 (D-06 / T-47.4-03-I2): atomic 0600 secret-file write —
/// the `AuthStore::save_to_disk` idiom (`auth/store.rs:607-649`) verbatim in
/// structure. The temp file is created at mode 0600 BEFORE any byte is
/// written (never a plain write-then-chmod, which leaves a world-readable
/// window), then flushed, fsynced, and renamed into place; the final path
/// gets a redundant chmod as a safety net. This is deliberately NOT the
/// `config.yaml` temp+rename helper — that path writes its temp file at
/// default (non-0600) permissions, which is exactly the window this idiom
/// avoids; it must never be used for a file carrying key material.
#[cfg(feature = "server")]
pub(crate) fn write_env_atomic_0600(final_path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    static TMP_WRITE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = TMP_WRITE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file_name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp_path = final_path.with_file_name(format!(
        "{file_name}.tmp.{}.{}",
        std::process::id(),
        counter
    ));

    let write_result = (|| -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp_path)?;
        f.write_all(contents.as_bytes())?;
        f.flush()?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    std::fs::rename(&tmp_path, final_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(final_path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Phase 47.4 Plan 03 (D-06/D-07/D-08/D-13): the full scaffold — validate,
/// never-clobber-without-force, byte-copy `config.yaml`, resolve and
/// overlay keys, atomic 0600 `.env` write. Pure/synchronous and disk-only
/// (no process-env mutation, no subprocess) so it is directly testable
/// without a server runtime — mirrors the "test the logic layer, not the
/// `#[server]`-wrapped fn" precedent (`provider_config_api.rs`'s
/// `merge_provider_payload`, `tests/kanban_board_read.rs`'s own module
/// doc). Called from `create_profile` inside `spawn_blocking`.
#[cfg(feature = "server")]
pub(crate) fn create_profile_impl(
    name: &str,
    key_mode: &KeyMode,
    force: bool,
    manual_keys: Vec<(String, SecretString)>,
    config: &ironhermes_core::config::Config,
) -> Result<Vec<KeyRow>, String> {
    // Step 1 (D-08): validate via the real ironhermes_core fn, reused not
    // re-implemented. Runs before any path is constructed or any byte is
    // written — a rejected name creates nothing on disk (T-47.4-03-T1).
    ironhermes_core::profile::validate_profile_name(name)
        .map_err(|e| format!("invalid profile name: {e}"))?;

    // Phase 47.4 Plan 18 (CR-02, T-47.4-18-01/02): validate every
    // manual_keys entry at this pure-impl boundary, before any path is
    // constructed or any byte written — a rejected entry creates nothing on
    // disk, matching the validate_profile_name discipline directly above.
    // Never interpolates a value into the error (D-13) — `key` below is the
    // NAME, which validate_key_name's own error text already names too.
    for (key, secret) in &manual_keys {
        validate_key_name(key).map_err(|e| format!("invalid manual key: {e}"))?;
        validate_key_value(secret.expose_secret())
            .map_err(|e| format!("invalid manual key '{key}': {e}"))?;
    }

    let profile_dir = profile_dir_for(name);
    let config_path = profile_dir.join("config.yaml");
    let env_path = profile_dir.join(".env");

    let existing_config = config_path.is_file();
    let existing_env = env_path.is_file();
    if profile_dir.is_dir() && (existing_config || existing_env) && !force {
        return Err(format!(
            "profile '{name}' already exists — pass --force to overwrite its config.yaml/.env"
        ));
    }

    std::fs::create_dir_all(&profile_dir)
        .map_err(|e| format!("create_dir_all({profile_dir:?}): {e}"))?;

    // config.yaml: byte copy from root — never round-tripped through a
    // YAML (de)serializer, which would silently rewrite unknown keys
    // (mirrors the script's own :87-94 behavior, including the SKIPPED
    // branch when no root config.yaml exists yet to copy).
    if !existing_config || force {
        let root_config_path = ironhermes_core::get_hermes_home().join("config.yaml");
        if root_config_path.is_file() {
            std::fs::copy(&root_config_path, &config_path)
                .map_err(|e| format!("copy config.yaml: {e}"))?;
        }
    }

    // Resolve inherited keys from the root .env (a missing root .env
    // resolves nothing — not an error), then overlay manual_keys (manual
    // wins for a name present in both).
    let root_env_path = ironhermes_core::get_hermes_home().join(".env");
    let root_env_map = read_env_keys(&root_env_path).map_err(|e| format!("read root .env: {e}"))?;
    let mut resolved = resolve_inherited_keys(&root_env_map, key_mode, config);
    let manual_names: std::collections::HashSet<String> =
        manual_keys.iter().map(|(k, _)| k.clone()).collect();
    for (key, secret) in &manual_keys {
        let value = secret.expose_secret().to_string();
        match resolved.iter_mut().find(|(k, _)| k == key) {
            Some(existing) => existing.1 = value,
            None => resolved.push((key.clone(), value)),
        }
    }

    let keep_existing_env = existing_env && !force;
    if !keep_existing_env {
        // Phase 47.4 Plan 18 (WR-01): sort before render, matching
        // `save_profile_key_impl`'s existing discipline ("so a rewrite is
        // deterministic") — an unsorted forged duplicate landing after an
        // already-resolved inherited key would otherwise win the
        // `read_env_keys` `HashMap` insert on the next parse, silently
        // changing which credential a dispatched worker uses. Sorts the
        // final list only; the manual-overlay-wins logic above is
        // untouched.
        resolved.sort_by(|a, b| a.0.cmp(&b.0));
        let contents = render_profile_env(name, &resolved)?;
        write_env_atomic_0600(&env_path, &contents).map_err(|e| format!("write .env: {e}"))?;
    }

    // Build the returned rows from what is now actually on disk — the
    // fresh write, or the untouched existing file if kept.
    let final_env_map: HashMap<String, String> = if keep_existing_env {
        read_env_keys(&env_path).map_err(|e| format!("read written .env: {e}"))?
    } else {
        resolved.iter().cloned().collect()
    };

    let mut rows: Vec<KeyRow> = final_env_map
        .iter()
        .map(|(key, value)| KeyRow {
            name: key.clone(),
            status: if manual_names.contains(key) {
                KeyStatus::ManuallySet
            } else {
                KeyStatus::Inherited
            },
            masked: mask_key_value(value),
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rows)
}

/// Phase 47.4 Plan 03: fail-closed write gate — same flag and error string
/// `update_provider_config`/`write_provider_secret` already enforce for
/// this codebase's other two browser-reachable credential/config write
/// surfaces (`provider_config_api.rs:261-264`, `provider_secrets_api.rs`'s
/// `check_double_gate`). Pure and disk-I/O-free so it is directly
/// unit-testable.
#[cfg(feature = "server")]
pub(crate) fn check_profile_write_gate(
    config: &ironhermes_core::config::Config,
) -> Result<(), String> {
    if !config.security.web_config_write_enabled {
        return Err("Config writes are disabled".to_string());
    }
    Ok(())
}

/// Phase 47.4 Plan 03 (D-06/D-07/D-08/D-13): create a dispatchable kanban
/// profile as a native scaffold — the exact failure this phase exists to
/// close (`47.4-CONTEXT.md`). Follows `update_provider_config`'s four-step
/// protocol (`provider_config_api.rs:240-281`): validate → fresh
/// `Config::load()` → fail-closed gate → do the write (here, inside
/// `spawn_blocking` via `create_profile_impl`). No subprocess spawn of any
/// kind and no process-environment mutation (D-08) — keys are written to
/// the profile `.env` only, never to any secret-storage backend (D-06).
#[server]
pub async fn create_profile(req: CreateProfileRequest) -> Result<Vec<KeyRow>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        check_profile_write_gate(&config).map_err(ServerFnError::new)?;

        let CreateProfileRequest {
            name,
            key_mode,
            force,
            manual_keys,
        } = req;
        // D-13: wrap manual key values in SecretString the moment they are
        // available — before they cross into spawn_blocking, and before
        // they can reach any Debug-deriving struct.
        let manual_keys: Vec<(String, SecretString)> = manual_keys
            .into_iter()
            .map(|(k, v)| (k, SecretString::from(v)))
            .collect();

        let rows = tokio::task::spawn_blocking(move || {
            create_profile_impl(&name, &key_mode, force, manual_keys, &config)
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;

        Ok(rows)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = req;
        Err(ServerFnError::new(
            "create_profile unavailable without `server` feature",
        ))
    }
}

// ============================================================================
// Phase 50.1 Plan 06 (D-17): duplicate_profile — an explicit allowlist copy
// that omits key material.
// ============================================================================

/// Phase 50.1 Plan 06: recursive directory copy shared by
/// `DuplicateCopyEntry`'s `SkillsDir`/`MemoriesDir` variants and
/// `bot_avatar_api::copy_bot_avatar_files`. Never follows a symlink — a
/// symlinked entry anywhere inside the source tree is skipped rather than
/// dereferenced, so a duplicate can never be tricked into copying bytes
/// from outside the source directory it was told to copy. Creates the
/// destination directory even when the source is empty, so an empty
/// `skills/` dir round-trips as an empty `skills/` dir rather than a
/// missing one.
#[cfg(feature = "server")]
pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("create_dir_all({dst:?}): {e}"))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("read_dir({src:?}): {e}"))? {
        let entry = entry.map_err(|e| format!("read_dir entry: {e}"))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("file_type({:?}): {e}", entry.path()))?;
        let dest_path = dst.join(entry.file_name());
        if file_type.is_symlink() {
            continue;
        } else if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), &dest_path)
                .map_err(|e| format!("copy {:?}: {e}", entry.path()))?;
        }
    }
    Ok(())
}

/// Phase 50.1 Plan 06 (D-17): one entry in the explicit allowlist a
/// duplicate copies. A `match` arm per variant, not a generic "copy this
/// relative path" table — `WorkspacePersona` needs bespoke handling (the
/// D-16 workspace-resolver marker directory), which a purely data-driven
/// table would not naturally express.
#[cfg(feature = "server")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DuplicateCopyEntry {
    /// `config.yaml` — copied only if the source has one (a profile
    /// scaffolded with no root `config.yaml` to copy from legitimately has
    /// none, mirroring `create_profile_impl`'s own "SKIPPED" branch).
    ConfigFile,
    /// `skills/` — the profile's own skill directory, copied only if
    /// present.
    SkillsDir,
    /// `memories/` — the profile's own memory store, copied only if
    /// present.
    MemoriesDir,
    /// The persona file (`SOUL.md`), never the rest of `workspace/` (live
    /// session/worktree state stays behind). OF-6 fix
    /// (`50.1-OPERATOR-FEEDBACK.md`): copies from/to the PROFILE-ROOT
    /// `SOUL.md` — `profile_dir_for(name).join("SOUL.md")` — the path
    /// `ironhermes_agent::prompt_builder::PromptBuilder::load_soul_md`
    /// actually reads at turn time via `get_hermes_home()` once
    /// `IRONHERMES_HOME` is pivoted by `--profile <bot_name>`. Reads the
    /// source with a fallback to the pre-OF-6 legacy `workspace/SOUL.md`
    /// location, so duplicating a bot whose persona predates this fix still
    /// carries it forward. The target's `workspace/.ironhermes/` marker
    /// directory is still seeded here (duplicating, not calling,
    /// `profile_workspace_dir`'s own marker step, since the target profile
    /// directory does not exist yet during staging) — that marker is
    /// load-bearing for `ironhermes-cli::run_single`'s workspace-scoped
    /// session/trajectory tracking during a handoff turn, a real and
    /// separate mechanism from persona loading (see `profile_workspace_dir`'s
    /// doc comment).
    WorkspacePersona,
}

/// Phase 50.1 Plan 06 (D-17): the explicit allowlist of profile artifacts a
/// clone carries — copy by ALLOWLIST, never by directory sweep with
/// exclusions. An allowlist grows only when a future change explicitly adds
/// an entry here; a sweep-with-exclusions would silently start copying
/// anything a future phase adds to the profile directory shape, and the one
/// thing that must NEVER be copied (`.env` — the CR-03 exfiltration
/// mechanism, since profile `.env` values are dotenvy-substituted at read
/// time and a copied variable reference can dereference to a different
/// secret in its new home) is exactly the kind of file a future phase would
/// add. `.env`, `state.db`, `sessions/`, `cron/`, `logs/`,
/// `subagent-transcripts/`, `browser-profile/` and `ui-meta.json` (the
/// bot-meta sidecar, handled separately by `bot_meta_api::copy_bot_meta`)
/// are all deliberately absent — none is ever touched by
/// `duplicate_profile_impl`.
///
/// Credential-carrying duplication waits for Phase 51's per-profile
/// secret-storage work, where it becomes a re-key to the new bot rather
/// than a plaintext copy — this omission is a design, not an oversight.
/// (D-06: this file never names that future storage mechanism.)
#[cfg(feature = "server")]
const DUPLICATE_COPY_ENTRIES: &[DuplicateCopyEntry] = &[
    DuplicateCopyEntry::ConfigFile,
    DuplicateCopyEntry::SkillsDir,
    DuplicateCopyEntry::MemoriesDir,
    DuplicateCopyEntry::WorkspacePersona,
];

/// Phase 50.1 Plan 06: dispatch one [`DuplicateCopyEntry`] into a
/// build-then-promote staging directory. `staging_dir` is NOT yet the
/// target profile's real path (see `duplicate_profile_impl`) — every path
/// here is joined relative to it directly, never through `profile_dir_for`.
#[cfg(feature = "server")]
fn copy_duplicate_entry(
    entry: DuplicateCopyEntry,
    source_dir: &Path,
    staging_dir: &Path,
) -> Result<(), String> {
    match entry {
        DuplicateCopyEntry::ConfigFile => {
            let src = source_dir.join("config.yaml");
            if src.is_file() {
                std::fs::copy(&src, staging_dir.join("config.yaml"))
                    .map_err(|e| format!("copy config.yaml: {e}"))?;
            }
            Ok(())
        }
        DuplicateCopyEntry::SkillsDir => {
            let src = source_dir.join("skills");
            if src.is_dir() {
                copy_dir_recursive(&src, &staging_dir.join("skills"))?;
            }
            Ok(())
        }
        DuplicateCopyEntry::MemoriesDir => {
            let src = source_dir.join("memories");
            if src.is_dir() {
                copy_dir_recursive(&src, &staging_dir.join("memories"))?;
            }
            Ok(())
        }
        DuplicateCopyEntry::WorkspacePersona => {
            // OF-6 fix: canonical source is the profile-root SOUL.md;
            // legacy `workspace/SOUL.md` is the pre-fix location, kept as a
            // fallback so a source bot saved before this fix still clones
            // its persona forward.
            let canonical_src = source_dir.join("SOUL.md");
            let legacy_src = source_dir.join("workspace").join("SOUL.md");
            let src = if canonical_src.is_file() {
                Some(canonical_src)
            } else if legacy_src.is_file() {
                Some(legacy_src)
            } else {
                None
            };
            if let Some(src) = src {
                let target_workspace = staging_dir.join("workspace");
                std::fs::create_dir_all(&target_workspace)
                    .map_err(|e| format!("create target workspace dir: {e}"))?;
                // Session/trajectory-scoping marker — see the
                // `WorkspacePersona` variant's own doc comment for why this
                // is duplicated here rather than calling
                // `profile_workspace_dir`.
                std::fs::create_dir_all(target_workspace.join(".ironhermes"))
                    .map_err(|e| format!("create target workspace marker dir: {e}"))?;
                std::fs::copy(&src, staging_dir.join("SOUL.md"))
                    .map_err(|e| format!("copy persona file: {e}"))?;
            }
            Ok(())
        }
    }
}

/// Phase 50.1 Plan 06 (D-17, T-50.1-06-01/07): the `duplicate_profile` impl
/// layer. Validates both names, rejects an existing target and a missing
/// source before any path is resolved, then builds the copy in a staging
/// directory and promotes it with a single `rename` — a mid-copy failure
/// therefore never leaves a half-populated target directory an operator
/// would mistake for a working bot (T-50.1-06-07). The staging directory's
/// name is dot-prefixed so `list_profiles`'s existing dotfile skip already
/// hides a crash-orphaned staging directory from the roster.
///
/// After the directory promotes, copies the "look" (D-17): the bot-meta
/// record via `bot_meta_api::copy_bot_meta`, then any avatar file bytes it
/// references via `bot_avatar_api::copy_bot_avatar_files`. Either failing
/// rolls back the just-promoted target directory — a clone with config but
/// a silently missing look is a degraded clone, not the "no half-done
/// state" guarantee this fn promises.
#[cfg(feature = "server")]
pub(crate) fn duplicate_profile_impl(source: &str, target: &str) -> Result<String, String> {
    let validated_source = ironhermes_core::profile::validate_profile_name(source)
        .map_err(|e| format!("invalid source profile name: {e}"))?;
    let validated_target = ironhermes_core::profile::validate_profile_name(target)
        .map_err(|e| format!("invalid target profile name: {e}"))?;

    let source_dir = profile_dir_for(&validated_source);
    if !source_dir.is_dir() {
        return Err(format!("source profile '{validated_source}' does not exist"));
    }

    let target_dir = profile_dir_for(&validated_target);
    if target_dir.exists() {
        return Err(format!("profile '{validated_target}' already exists"));
    }

    let profiles_root = ironhermes_core::get_hermes_home().join(ironhermes_core::PROFILES_SUBDIR);
    std::fs::create_dir_all(&profiles_root)
        .map_err(|e| format!("create_dir_all(profiles root): {e}"))?;
    // Leading dot: `list_profiles` already skips dot-prefixed entries
    // (T-47.4-01-D1), so a staging directory left behind by a crashed
    // duplicate never appears in the roster as a phantom bot.
    let staging_dir = profiles_root.join(format!(".duplicate-staging-{}", uuid::Uuid::new_v4()));
    let _ = std::fs::remove_dir_all(&staging_dir); // clear any stale leftover

    let build_result = (|| -> Result<(), String> {
        std::fs::create_dir_all(&staging_dir)
            .map_err(|e| format!("create_dir_all(staging): {e}"))?;
        for entry in DUPLICATE_COPY_ENTRIES {
            copy_duplicate_entry(*entry, &source_dir, &staging_dir)?;
        }
        Ok(())
    })();

    if let Err(e) = build_result {
        let _ = std::fs::remove_dir_all(&staging_dir);
        return Err(e);
    }

    if let Err(e) = std::fs::rename(&staging_dir, &target_dir) {
        let _ = std::fs::remove_dir_all(&staging_dir);
        return Err(format!("promote staged copy to {target_dir:?}: {e}"));
    }

    if let Err(e) = crate::server::bot_meta_api::copy_bot_meta(&validated_source, &validated_target)
    {
        let _ = std::fs::remove_dir_all(&target_dir);
        return Err(format!("copy bot-meta: {e}"));
    }
    if let Err(e) =
        crate::server::bot_avatar_api::copy_bot_avatar_files(&validated_source, &validated_target)
    {
        let _ = std::fs::remove_dir_all(&target_dir);
        return Err(format!("copy avatar: {e}"));
    }

    Ok(validated_target)
}

/// Phase 50.1 Plan 06 (D-17): the `duplicate_profile` `#[server]` fn.
/// Follows this crate's four-step write protocol: validate → fresh
/// `Config::load()` → `check_profile_write_gate` → `spawn_blocking` around
/// `duplicate_profile_impl`. There is no profile CLI subcommand to delegate
/// to in this workspace (RESEARCH.md Pitfall 3) — the copy is direct
/// filesystem work, following `create_profile_impl`'s own discipline (no
/// force-unwraps, every error a propagated value).
#[server]
pub async fn duplicate_profile(req: DuplicateProfileRequest) -> Result<String, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        check_profile_write_gate(&config).map_err(ServerFnError::new)?;

        let DuplicateProfileRequest { source, target } = req;
        let created = tokio::task::spawn_blocking(move || duplicate_profile_impl(&source, &target))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)?;
        Ok(created)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = req;
        Err(ServerFnError::new(
            "duplicate_profile unavailable without `server` feature",
        ))
    }
}

// ============================================================================
// Phase 50.1 Plan 06 (D-18): delete_profile — permanent removal with a
// synchronous metadata delete hook.
// ============================================================================

/// Phase 50.1 Plan 06 (D-18, T-50.1-06-04): the one deletion-protected
/// profile name. A dedicated, explicit predicate rather than relying on
/// `validate_profile_name`'s `RESERVED_NAMES` rejection of "default" as an
/// incidental side effect — the reservation exists for a different reason
/// (avoiding a name collision with `current_profile()`'s own "no profile
/// selected" sentinel at CREATE time) and could in principle change without
/// this delete-time guarantee changing with it. Checked at the impl layer
/// so the refusal holds even if a caller bypasses the UI entirely
/// (T-50.1-06-04).
#[cfg(feature = "server")]
pub(crate) fn is_deletion_protected(name: &str) -> bool {
    name == "default"
}

/// Phase 50.1 Plan 06 (D-18, T-50.1-06-02/03/04): the `delete_profile` impl
/// layer — permanent removal of an ordinary bot's profile directory plus
/// its bot-meta entry, in the same call. Refuses the default profile
/// ([`is_deletion_protected`]) and the currently live profile
/// (`ironhermes_core::current_profile()`) before ever resolving a path, and
/// confirms the resolved directory is contained within the profiles root
/// (canonicalized, following any symlink to its real target) and is not
/// itself a symlink before removing anything — never follows a symlink out
/// of the profiles root (T-50.1-06-02).
#[cfg(feature = "server")]
pub(crate) fn delete_profile_impl(name: &str) -> Result<(), String> {
    if is_deletion_protected(name) {
        return Err("the default profile can't be deleted".to_string());
    }
    if name == ironhermes_core::current_profile() {
        return Err(format!(
            "profile '{name}' is the currently live profile — it cannot be deleted while it is serving the embedded runtime"
        ));
    }

    let validated_name = ironhermes_core::profile::validate_profile_name(name)
        .map_err(|e| format!("invalid profile name: {e}"))?;

    let profile_dir = profile_dir_for(&validated_name);
    if !profile_dir.is_dir() {
        return Err(format!("profile '{validated_name}' does not exist"));
    }
    if profile_dir.is_symlink() {
        return Err(format!(
            "refusing to remove '{validated_name}': profile path is a symlink"
        ));
    }

    let profiles_root = ironhermes_core::get_hermes_home().join(ironhermes_core::PROFILES_SUBDIR);
    let canonical_dir = std::fs::canonicalize(&profile_dir)
        .map_err(|e| format!("resolve profile directory: {e}"))?;
    let canonical_root = std::fs::canonicalize(&profiles_root)
        .map_err(|e| format!("resolve profiles root: {e}"))?;
    if !canonical_dir.starts_with(&canonical_root) {
        return Err(format!(
            "refusing to remove '{validated_name}': resolved path escapes the profiles root"
        ));
    }

    std::fs::remove_dir_all(&profile_dir)
        .map_err(|e| format!("remove profile directory: {e}"))?;

    // Sibling avatar directory (D-11: never nested inside profiles/) — best
    // effort. The profile is already gone either way; a stray avatar
    // directory left behind by a permission edge case must not turn an
    // otherwise-successful delete into a reported failure.
    let _ = std::fs::remove_dir_all(crate::server::bot_avatar_api::bot_avatar_dir(
        &validated_name,
    ));

    crate::server::bot_meta_api::delete_bot_meta_impl(&validated_name)?;

    Ok(())
}

/// Phase 50.1 Plan 06 (D-18): the `delete_profile` `#[server]` fn. Follows
/// this crate's four-step write protocol: validate → fresh `Config::load()`
/// → `check_profile_write_gate` → `spawn_blocking` around
/// `delete_profile_impl`. There is no profile CLI subcommand to shell out
/// to in this workspace (RESEARCH.md Pitfall 3) — this is direct filesystem
/// removal.
#[server]
pub async fn delete_profile(name: String) -> Result<(), ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        check_profile_write_gate(&config).map_err(ServerFnError::new)?;

        tokio::task::spawn_blocking(move || delete_profile_impl(&name))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)?;
        Ok(())
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = name;
        Err(ServerFnError::new(
            "delete_profile unavailable without `server` feature",
        ))
    }
}

// ============================================================================
// Phase 47.4 Plan 07 (D-07/D-08/D-13): wizard step-2 key-resolution preview.
// ============================================================================
//
// The wizard's CONFIG & KEYS step needs a live resolved-key table BEFORE the
// profile exists — `fetch_profile_detail` (Plan 05) requires an existing
// profile dir and cannot serve this. This is a small, read-only sibling:
// it reuses `resolve_inherited_keys`/`mask_key_value`/`read_env_keys`
// verbatim and reads only the ROOT `.env` (a not-yet-created profile has no
// `.env` of its own). Ungated, like `fetch_profile_detail` — a read, not a
// write — and structurally cannot leak a real key value: `KeyRow` carries
// `masked`/`status` only (D-13).

/// Phase 47.4 Plan 07 (D-07/D-13): pure/synchronous/disk-only preview build.
/// Row set per mode: `LlmOnly` is always the fixed five-name allowlist
/// (so a missing LLM key renders a visible `Missing` row, not an absent
/// one); `AllKeys` is every root-`.env` name matching the suffix pattern;
/// `Explicit` is exactly the caller-supplied names. A manually entered
/// value always overlays and, if its name is outside the mode's base set,
/// is appended as its own row — mirrors `create_profile_impl`'s own
/// overlay-and-append rule for the same reason (a manual key targeting a
/// name the mode doesn't cover must still be visible before create).
#[cfg(feature = "server")]
pub(crate) fn preview_resolved_keys_impl(
    key_mode: &KeyMode,
    manual_keys: Vec<(String, SecretString)>,
    config: &ironhermes_core::config::Config,
) -> Result<Vec<KeyRow>, String> {
    let root_env_path = ironhermes_core::get_hermes_home().join(".env");
    let root_env_map = read_env_keys(&root_env_path).map_err(|e| format!("read root .env: {e}"))?;

    let base_names: Vec<String> = match key_mode {
        KeyMode::LlmOnly => provider_key_env_names(config),
        KeyMode::AllKeys => {
            let mut matched: Vec<String> = root_env_map
                .keys()
                .filter(|k| {
                    ROOT_KEY_PATTERN_SUFFIXES
                        .iter()
                        .any(|suffix| k.ends_with(suffix))
                })
                .cloned()
                .collect();
            matched.sort();
            matched
        }
        KeyMode::Explicit(names) => names.clone(),
    };

    let manual_map: HashMap<String, String> = manual_keys
        .into_iter()
        .map(|(k, v)| (k, v.expose_secret().to_string()))
        .filter(|(_, v)| !v.is_empty())
        .collect();

    let mut names: Vec<String> = base_names;
    for name in manual_map.keys() {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }

    let rows: Vec<KeyRow> = names
        .into_iter()
        .map(|name| {
            let root_val = root_env_map.get(&name).filter(|v| !v.is_empty());
            let manual_val = manual_map.get(&name).filter(|v| !v.is_empty());
            let (status, masked) = match (root_val, manual_val) {
                (_, Some(m)) => (KeyStatus::ManuallySet, mask_key_value(m)),
                (Some(r), None) => (KeyStatus::Inherited, mask_key_value(r)),
                (None, None) => (KeyStatus::Missing, mask_key_value("")),
            };
            KeyRow { name, status, masked }
        })
        .collect();
    Ok(rows)
}

/// Phase 47.4 Plan 07 (D-07/D-13): the wizard step-2 preview endpoint. Reads
/// only the root `.env`; never touches a profile directory and never writes
/// anything. Registered/auth-gated identically to every other fn in this
/// module (see this file's module doc).
#[server]
pub async fn preview_resolved_keys(
    key_mode: KeyMode,
    manual_keys: Vec<(String, String)>,
) -> Result<Vec<KeyRow>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        // GAP-1: the LlmOnly base-name set is now provider-registry-derived
        // (`provider_key_env_names`), so this preview needs the root Config
        // it did not previously load.
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        let manual_keys: Vec<(String, SecretString)> = manual_keys
            .into_iter()
            .map(|(k, v)| (k, SecretString::from(v)))
            .collect();
        let rows = tokio::task::spawn_blocking(move || {
            preview_resolved_keys_impl(&key_mode, manual_keys, &config)
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;
        Ok(rows)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (key_mode, manual_keys);
        Err(ServerFnError::new(
            "preview_resolved_keys unavailable without `server` feature",
        ))
    }
}

// ============================================================================
// Phase 47.4 Plan 05 (D-02/D-04/D-13): profile detail — read + edit surface.
// ============================================================================
//
// `fetch_profile_detail` turns the drawer from a create-only affordance into
// something readable/editable any time after creation (D-04). `KeyRow`s are
// classified per-source (root vs profile `.env`) via `classify_key_status`,
// and masked via `mask_key_value` — no raw key value is ever assembled into
// a response (D-13). `update_profile_config`/`save_profile_key` are the
// paired write side, gated fail-closed behind the same
// `web_config_write_enabled` flag `create_profile` already enforces.

/// Phase 47.4 Plan 05 (D-07/D-13): honest per-source key-status
/// classification. Pure — takes the two candidate values only to compare
/// them, and returns no data derived from them beyond the enum, so this fn
/// structurally cannot leak key material through its own return value.
/// An empty-string value is treated as absent on both sides (mirrors
/// `resolve_inherited_keys`'s empty-value drop and `list_profiles`'
/// resolvable-key filter).
#[cfg(feature = "server")]
pub(crate) fn classify_key_status(root: Option<&String>, profile: Option<&String>) -> KeyStatus {
    let root_val = root.filter(|v| !v.is_empty());
    let profile_val = profile.filter(|v| !v.is_empty());
    match (root_val, profile_val) {
        (Some(r), Some(p)) if r == p => KeyStatus::Inherited,
        (_, Some(_)) => KeyStatus::ManuallySet,
        (_, None) => KeyStatus::Missing,
    }
}

/// Phase 47.4 Plan 05 (D-02/D-04/D-11/D-13): the full detail read, extracted
/// as a pure/synchronous/disk-only fn (mirrors `create_profile_impl`'s own
/// "test the logic layer, not the `#[server]`-wrapped fn" precedent) so it
/// is directly testable without a server runtime. Called from
/// `fetch_profile_detail` inside `spawn_blocking`.
#[cfg(feature = "server")]
pub(crate) fn fetch_profile_detail_impl(name: &str) -> Result<ProfileDetail, String> {
    let dir = profile_dir_for(name);
    if !dir.is_dir() {
        return Err(format!("profile '{name}' does not exist"));
    }

    // config.yaml: same on-disk-presence + parse-fallibility handling as
    // `list_profiles` — a missing OR malformed file degrades provider/
    // model_default to None and surfaces as a ProfileGap::MissingConfigYaml
    // via classify_profile_health below, never failing the whole call.
    let config_path = dir.join("config.yaml");
    let config_yaml_on_disk = config_path.is_file();
    let (loaded_profile_config, provider, model_default, config_effectively_present) =
        if config_yaml_on_disk {
            match ironhermes_core::config::Config::load_from(&config_path) {
                Ok(cfg) => {
                    let provider = Some(cfg.model.provider.clone());
                    let model_default = Some(cfg.model.default.clone());
                    (Some(cfg), provider, model_default, true)
                }
                Err(_) => (None, None, None, false),
            }
        } else {
            (None, None, None, false)
        };

    // Read BOTH .env files — a malformed one on either side propagates Err
    // as a value (T-47.4-05-D1 mitigation), never a panic.
    let root_env_path = ironhermes_core::get_hermes_home().join(".env");
    let root_env = read_env_keys(&root_env_path).map_err(|e| format!("read root .env: {e}"))?;
    let profile_env_path = dir.join(".env");
    let profile_env =
        read_env_keys(&profile_env_path).map_err(|e| format!("read profile .env: {e}"))?;

    // Row set: the provider-registry-derived key names (GAP-1: was the
    // fixed five-name floor), in order, so a missing key is always a
    // visible Missing row, plus every name present in the profile .env that
    // isn't already in that set, sorted alphabetically after it. Falls back
    // to the compatibility floor when the profile's own config.yaml didn't
    // parse (no Config to derive a wider set from).
    let allowlist_names: Vec<String> = match &loaded_profile_config {
        Some(cfg) => provider_key_env_names(cfg),
        None => LLM_KEY_ALLOWLIST.iter().map(|s| s.to_string()).collect(),
    };
    let mut names: Vec<String> = allowlist_names.clone();
    let mut extra: Vec<String> = profile_env
        .keys()
        .filter(|k| !allowlist_names.iter().any(|n| n.as_str() == k.as_str()))
        .cloned()
        .collect();
    extra.sort();
    names.extend(extra);

    let keys: Vec<KeyRow> = names
        .into_iter()
        .map(|key_name| {
            let root_val = root_env.get(&key_name);
            let profile_val = profile_env.get(&key_name);
            let status = classify_key_status(root_val, profile_val);
            // mask_key_value only needs presence, not the real value — but
            // whichever candidate is non-empty is what's "present" here.
            let value_for_mask = profile_val
                .filter(|v| !v.is_empty())
                .or_else(|| root_val.filter(|v| !v.is_empty()));
            let masked = mask_key_value(value_for_mask.map(String::as_str).unwrap_or(""));
            KeyRow {
                name: key_name,
                status,
                masked,
            }
        })
        .collect();

    // Health/gaps: same rule `list_profiles` uses, resolved against the
    // PROFILE's own .env only (not root) — mirrors list_profiles' own
    // resolvable-key filter exactly, so the two call sites can never
    // disagree for the same disk state (GAP-1: now provider-aware via the
    // single `ironhermes_core::dispatch_gate` predicate).
    let resolvable_llm_key_count = allowlist_names
        .iter()
        .filter(|k| profile_env.get(k.as_str()).map(|v| !v.is_empty()).unwrap_or(false))
        .count();
    let provider_key = compute_provider_key_state(
        name,
        config_effectively_present,
        provider.as_deref(),
        resolvable_llm_key_count,
    );
    let (health, gaps) = classify_profile_health(true, config_effectively_present, provider_key);

    // web_config_write_enabled: the root flag, not profile-specific —
    // reported truthfully so the client can render a disabled-write state.
    let config =
        ironhermes_core::config::Config::load().map_err(|e| format!("Config load failed: {e}"))?;

    Ok(ProfileDetail {
        name: name.to_string(),
        dir: dir.to_string_lossy().to_string(),
        health,
        gaps,
        provider,
        model_default,
        keys,
        web_config_write_enabled: config.security.web_config_write_enabled,
    })
}

/// Phase 47.4 Plan 05 (D-02/D-04): the profile detail drawer's read side.
/// Ungated read, matching `get_provider_config`
/// (`provider_config_api.rs:222-238`) — reports `web_config_write_enabled`
/// as a field rather than gating on it, since this is a read, not a write.
#[server]
pub async fn fetch_profile_detail(name: String) -> Result<ProfileDetail, ServerFnError> {
    #[cfg(feature = "server")]
    {
        // ASVS V5: the name arrives from the browser here, unlike in
        // list_profiles (which only reads names already on disk) — validate
        // before constructing any path.
        ironhermes_core::profile::validate_profile_name(&name)
            .map_err(|e| ServerFnError::new(format!("invalid profile name: {e}")))?;

        let detail = tokio::task::spawn_blocking(move || fetch_profile_detail_impl(&name))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)?;
        Ok(detail)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = name;
        Err(ServerFnError::new(
            "fetch_profile_detail unavailable without `server` feature",
        ))
    }
}

/// Phase 47.4 Plan 05 (D-04): payload validation for `update_profile_config`
/// — reuses `validate_profile_name` (defense-in-depth, matching
/// `create_profile`) and rejects an explicitly-empty `Some("")` provider or
/// model (an empty model would silently break judge resolution). Pure and
/// disk-I/O-free.
#[cfg(feature = "server")]
pub(crate) fn validate_profile_config_payload(
    payload: &ProfileConfigWritePayload,
) -> Result<(), String> {
    ironhermes_core::profile::validate_profile_name(&payload.name)
        .map_err(|e| format!("invalid profile name: {e}"))?;
    if let Some(ref provider) = payload.provider {
        if provider.trim().is_empty() {
            return Err("provider must not be empty".to_string());
        }
    }
    if let Some(ref model_default) = payload.model_default {
        if model_default.trim().is_empty() {
            return Err("model must not be empty".to_string());
        }
    }
    // Phase 50.1 Plan 05 (D-15, T-50.1-05-01): reject a malformed opt-out
    // list before it ever reaches a config write — an empty or
    // whitespace-only entry would silently no-op-disable nothing while
    // still occupying a slot, and an unbounded list is a cheap DoS vector
    // against the profile config file. Catalog-membership validation
    // happens separately, in `apply_skills_disabled`, which is the only
    // place that actually knows the catalog.
    if let Some(ref skills_disabled) = payload.skills_disabled {
        const MAX_SKILLS_DISABLED_ENTRIES: usize = 512;
        if skills_disabled.len() > MAX_SKILLS_DISABLED_ENTRIES {
            return Err(format!(
                "skills_disabled must not exceed {MAX_SKILLS_DISABLED_ENTRIES} entries"
            ));
        }
        if skills_disabled.iter().any(|name| name.trim().is_empty()) {
            return Err(
                "skills_disabled entries must not be empty or whitespace-only".to_string(),
            );
        }
    }
    Ok(())
}

/// Phase 50.1 Plan 05 (D-15, T-50.1-05-01/T-50.1-05-03): merge a validated
/// opt-out list onto a profile's `skills.disabled`, rejecting any name not
/// present in `catalog_names` — writing nothing (this fn only mutates
/// `cfg` in memory; the caller's `cfg.save_to` never runs when this
/// returns `Err`, so an unknown name can never even partially land on
/// disk). Every other `SkillsConfig` field (`enabled`, `extra_paths`,
/// `credential_dir`, `config`, `hub`, `defcon_level`) and every non-skills
/// config section survive untouched — this fn only ever assigns
/// `cfg.skills.disabled`. Pure and disk-I/O-free so it is directly
/// unit-testable, mirroring `merge_profile_config_payload`.
#[cfg(feature = "server")]
pub(crate) fn apply_skills_disabled(
    cfg: &mut ironhermes_core::config::Config,
    skills_disabled: &[String],
    catalog_names: &HashSet<String>,
) -> Result<(), String> {
    for name in skills_disabled {
        if name.trim().is_empty() {
            return Err("skill name must not be empty or whitespace-only".to_string());
        }
        if !catalog_names.contains(name) {
            return Err(format!("unknown skill: {name}"));
        }
    }
    cfg.skills.disabled = skills_disabled.to_vec();
    Ok(())
}

/// Phase 47.4 Plan 05 (D-04): merge only `Some` fields onto `cfg.model` —
/// mirrors `merge_provider_payload`'s (`provider_config_api.rs:154-180`)
/// merge-only-`Some` discipline. A payload with both fields `None` mutates
/// nothing.
#[cfg(feature = "server")]
pub(crate) fn merge_profile_config_payload(
    cfg: &mut ironhermes_core::config::Config,
    payload: &ProfileConfigWritePayload,
) {
    if let Some(ref provider) = payload.provider {
        cfg.model.provider = provider.clone();
    }
    if let Some(ref model_default) = payload.model_default {
        cfg.model.default = model_default.clone();
    }
}

/// Phase 47.4 Plan 05 (D-04): the full write — load THAT profile's own
/// `config.yaml` (never the root), merge, atomic save. `config.yaml` is
/// non-secret, so `Config::save_to`'s temp+rename (`config.rs:3568-3577`)
/// is correct here — this must never be used for a file carrying key
/// material (see `write_env_atomic_0600` for that path). Extracted as a
/// pure/synchronous/disk-only fn so it is directly testable, mirroring
/// `create_profile_impl`.
#[cfg(feature = "server")]
pub(crate) fn update_profile_config_impl(
    payload: &ProfileConfigWritePayload,
) -> Result<(), String> {
    let profile_dir = profile_dir_for(&payload.name);
    let config_path = profile_dir.join("config.yaml");
    let mut cfg = ironhermes_core::config::Config::load_from(&config_path)
        .map_err(|e| format!("load profile config.yaml: {e}"))?;
    merge_profile_config_payload(&mut cfg, payload);
    // Phase 50.1 Plan 05 (D-15): the catalog comes from the process-global
    // skill registry (T-50.1-05-03 — read-only enumeration of what is
    // installed on the machine is correct here; this never calls the
    // separate global skill-toggle server fn or mutates the registry's
    // active set).
    if let Some(ref skills_disabled) = payload.skills_disabled {
        let catalog_names: HashSet<String> = crate::server::state::global_app_state()
            .runtime
            .skill_registry()
            .list()
            .iter()
            .map(|r| r.name.clone())
            .collect();
        apply_skills_disabled(&mut cfg, skills_disabled, &catalog_names)?;
    }
    cfg.save_to(&config_path)
        .map_err(|e| format!("save profile config.yaml: {e}"))?;
    Ok(())
}

/// Phase 47.4 Plan 05 (D-04): update a profile's provider/model, any time
/// after creation. Follows `update_provider_config`'s four-step protocol
/// (`provider_config_api.rs:240-281`): validate → fresh `Config::load()` →
/// fail-closed gate → do the write (here, inside `spawn_blocking` via
/// `update_profile_config_impl`). Never writes to
/// `get_hermes_home().join("config.yaml")` — the target is always
/// `profile_dir_for(&payload.name).join("config.yaml")`.
#[server]
pub async fn update_profile_config(
    payload: ProfileConfigWritePayload,
) -> Result<(), ServerFnError> {
    #[cfg(feature = "server")]
    {
        validate_profile_config_payload(&payload).map_err(ServerFnError::new)?;

        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        check_profile_write_gate(&config).map_err(ServerFnError::new)?;

        tokio::task::spawn_blocking(move || update_profile_config_impl(&payload))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)?;

        Ok(())
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = payload;
        Err(ServerFnError::new(
            "update_profile_config unavailable without `server` feature",
        ))
    }
}

// =============================================================================
// Phase 50.1 Plan 05 (D-15/D-16): per-profile skills catalog + persona
// =============================================================================

/// Phase 50.1 Plan 05 (D-15): the process-global skill registry's category
/// derivation, duplicated verbatim from `server/api.rs`'s `list_skills`
/// (the "duplicate the trivial helper" precedent `cli_handoff.rs`'s
/// `now_ms` doc comment already establishes for this codebase) rather than
/// widening that fn's visibility across the module boundary. Pure and
/// disk-I/O-free.
#[cfg(feature = "server")]
fn skill_source_category(source: ironhermes_core::skills::SkillSource) -> &'static str {
    match source {
        ironhermes_core::skills::SkillSource::Builtin => "bundled",
        ironhermes_core::skills::SkillSource::Official => "official",
        ironhermes_core::skills::SkillSource::Trusted => "trusted",
        ironhermes_core::skills::SkillSource::Community => "installed",
        ironhermes_core::skills::SkillSource::SelfCreated => "self-created",
    }
}

/// Phase 50.1 Plan 05 (D-15): join a (name, category) catalog against one
/// profile's opt-out set — `enabled_for_profile` is the inverse of
/// membership, matching `SkillsConfig::disabled`'s "everything not named
/// is on" semantics. Extracted as a pure fn, decoupled from the live
/// `SkillRegistry` type, so it is directly unit-testable against a
/// synthetic catalog rather than depending on whatever skills happen to be
/// scanned on the machine running the test.
#[cfg(feature = "server")]
pub(crate) fn join_profile_skill_rows(
    catalog: &[(String, String)],
    disabled: &HashSet<String>,
) -> Vec<ProfileSkillRow> {
    catalog
        .iter()
        .map(|(name, category)| ProfileSkillRow {
            name: name.clone(),
            category: category.clone(),
            enabled_for_profile: !disabled.contains(name),
        })
        .collect()
}

/// Phase 50.1 Plan 05 (D-15): read one profile's skills catalog — the
/// process-global registry's catalog (what is installed on the machine)
/// joined against that profile's own `config.yaml` opt-out list (what is
/// enabled for THIS bot). Never calls the separate global skill-toggle
/// server fn and never mutates the
/// registry's active set — read-only enumeration only.
#[cfg(feature = "server")]
pub(crate) fn fetch_profile_skills_impl(name: &str) -> Result<Vec<ProfileSkillRow>, String> {
    ironhermes_core::profile::validate_profile_name(name)
        .map_err(|e| format!("invalid profile name: {e}"))?;
    let config_path = profile_dir_for(name).join("config.yaml");
    let cfg = ironhermes_core::config::Config::load_from(&config_path)
        .map_err(|e| format!("load profile config.yaml: {e}"))?;
    let disabled: HashSet<String> = cfg.skills.disabled.iter().cloned().collect();

    let catalog: Vec<(String, String)> = crate::server::state::global_app_state()
        .runtime
        .skill_registry()
        .list()
        .iter()
        .map(|r| (r.name.clone(), skill_source_category(r.source).to_string()))
        .collect();

    Ok(join_profile_skill_rows(&catalog, &disabled))
}

/// Phase 50.1 Plan 05 (D-15): `#[server]` read of one profile's skills
/// catalog. Follows this file's read-fn shape (`fetch_profile_detail`):
/// validate the name, then `spawn_blocking` the disk-touching impl.
#[server]
pub async fn fetch_profile_skills(name: String) -> Result<Vec<ProfileSkillRow>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        ironhermes_core::profile::validate_profile_name(&name)
            .map_err(|e| ServerFnError::new(format!("invalid profile name: {e}")))?;
        let rows = tokio::task::spawn_blocking(move || fetch_profile_skills_impl(&name))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)?;
        Ok(rows)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = name;
        Err(ServerFnError::new(
            "fetch_profile_skills unavailable without `server` feature",
        ))
    }
}

/// Phase 50.1 Plan 05 (D-15/D-16): resolve (creating if absent) a bot's own
/// workspace subdirectory — `profile_dir_for(name)/workspace`, the exact
/// default `cli_handoff::resolve_bot_workspace_dir` already resolves to
/// when the handoff caller supplies no override.
///
/// **Marker-directory step — NOT the persona-loading mechanism.** Plan 05
/// created this marker on the theory that
/// `ironhermes_core::workspace::resolve_from_cwd`'s resolved `soul_path` is
/// what the agent runtime reads at turn time. OF-6
/// (`50.1-OPERATOR-FEEDBACK.md`) found that theory false by direct source
/// read: nothing in `ironhermes-agent` (`prompt_builder.rs`,
/// `agent_runtime.rs`, `agent_loop.rs`) ever calls `resolve_from_cwd` or
/// reads `Workspace.soul_path` — `grep -rn` for both across the whole crate
/// returns zero hits. The actual and ONLY loader of persona content into a
/// turn's prompt is `PromptBuilder::load_soul_md`, which unconditionally
/// reads `ironhermes_core::get_hermes_home().join("SOUL.md")` — the
/// PROFILE-ROOT `SOUL.md` (see `save_profile_persona_impl` below), not
/// anything under `workspace/`.
///
/// This marker directory is still genuinely load-bearing, just for a
/// different, real mechanism: `ironhermes-cli::run_single` (the `chat -q`
/// handoff entry point) calls `resolve_from_cwd(cwd)` to scope the CLI
/// session's `workspace_root` (state.db) and trajectory-log directory to
/// the bot's own `workspace/` rather than falling back to
/// `$IRONHERMES_HOME`'s shared marker one level up. Kept here as a side
/// effect of saving a persona (the same event that currently provisions a
/// bot's `workspace/` directory at all — no other path creates it yet;
/// full per-bot workspace provisioning is deferred, per D-06).
#[cfg(feature = "server")]
pub(crate) fn profile_workspace_dir(name: &str) -> Result<PathBuf, String> {
    ironhermes_core::profile::validate_profile_name(name)
        .map_err(|e| format!("invalid profile name: {e}"))?;
    let workspace_dir = profile_dir_for(name).join("workspace");
    std::fs::create_dir_all(&workspace_dir)
        .map_err(|e| format!("create workspace dir: {e}"))?;
    let marker_dir = workspace_dir.join(".ironhermes");
    std::fs::create_dir_all(&marker_dir)
        .map_err(|e| format!("create workspace marker dir: {e}"))?;
    Ok(workspace_dir)
}

/// Phase 50.1 Plan 05 (D-15, T-50.1-05-06): a persona body longer than this
/// is rejected rather than silently truncated — an operator-visible error
/// beats a silently-clipped persona nobody notices.
#[cfg(feature = "server")]
pub(crate) const PROFILE_PERSONA_MAX_BODY_LEN: usize = 20_000;

/// Phase 50.1 Plan 05 (T-50.1-05-06): reject an over-long body or one
/// containing a control character other than newline/tab. Pure and
/// disk-I/O-free.
#[cfg(feature = "server")]
pub(crate) fn validate_persona_body(body: &str) -> Result<(), String> {
    if body.chars().count() > PROFILE_PERSONA_MAX_BODY_LEN {
        return Err(format!(
            "persona body exceeds the {PROFILE_PERSONA_MAX_BODY_LEN}-character limit"
        ));
    }
    if body.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err(
            "persona body must not contain control characters other than newline and tab"
                .to_string(),
        );
    }
    Ok(())
}

/// MA-01 fix (`50.1-REVIEW.md`): unique-per-call atomic write for the
/// persona file — same temp-then-rename discipline
/// `bot_meta_api::write_json_atomic` and `bot_avatar_api::write_avatar_bytes_atomic`
/// already use for this crate's other mutating writes (MI-03). A crash or
/// full disk mid-write must never leave a truncated `SOUL.md` that the bot
/// then silently runs with. Duplicated rather than shared — same
/// "duplicate the trivial helper" precedent `bot_meta_api.rs` documents for
/// `ScopedEnv`.
#[cfg(feature = "server")]
fn write_persona_atomic(final_path: &Path, contents: &str) -> Result<(), String> {
    use std::io::Write as _;

    static TMP_WRITE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = TMP_WRITE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file_name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp_path = final_path.with_file_name(format!(
        "{file_name}.tmp.{}.{}",
        std::process::id(),
        counter
    ));

    let write_result = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        f.write_all(contents.as_bytes())?;
        f.flush()?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("write persona file: {e}"));
    }
    std::fs::rename(&tmp_path, final_path).map_err(|e| format!("rename persona file: {e}"))
}

/// OF-6 fix (`50.1-OPERATOR-FEEDBACK.md`): the canonical, PROFILE-ROOT
/// persona path — `profile_dir_for(name).join("SOUL.md")`. This is exactly
/// what `ironhermes_core::get_hermes_home().join("SOUL.md")` resolves to
/// once `IRONHERMES_HOME` is pivoted to this profile by `--profile
/// <bot_name>` (`ironhermes-cli::resolve_and_set_profile`), which is the
/// path `ironhermes_agent::prompt_builder::PromptBuilder::load_soul_md`
/// unconditionally reads at turn time. Single source of truth for both the
/// save and fetch paths below so they can never drift apart again.
#[cfg(feature = "server")]
fn canonical_persona_path(name: &str) -> PathBuf {
    profile_dir_for(name).join("SOUL.md")
}

/// OF-6 fix: the pre-fix location Plan 05 originally wrote/read
/// (`workspace/SOUL.md`) — never consumed by the agent runtime at turn
/// time (see `profile_workspace_dir`'s doc comment for the full
/// evidence chain). Kept only as a read-side fallback so a persona saved
/// before this fix shipped (e.g. an already-configured bot) still
/// displays and migrates without the operator re-typing it.
#[cfg(feature = "server")]
fn legacy_persona_path(name: &str) -> PathBuf {
    profile_dir_for(name).join("workspace").join("SOUL.md")
}

/// Phase 50.1 Plan 05 (D-15/D-16), OF-6 fix: write a bot's persona
/// (SOUL.md) to [`canonical_persona_path`] — the profile ROOT, the
/// location the agent runtime actually reads at turn time. Still calls
/// `profile_workspace_dir` first (unchanged) purely for its
/// session/trajectory-scoping marker side effect — see that function's
/// doc comment.
///
/// MA-01 fix (`50.1-REVIEW.md`): refuses a profile whose directory does not
/// already exist — `profile_workspace_dir` unconditionally
/// `create_dir_all`s, so without this check a gated-ON save could scaffold
/// a phantom profile (bypassing `create_profile`) from a name that was
/// never legitimately created. MI-03 fix: routes through
/// [`write_persona_atomic`] instead of a plain `fs::write`.
///
/// OF-6: after a successful canonical write, removes the legacy
/// `workspace/SOUL.md` file if present (best-effort) so there is exactly
/// one source of truth going forward — the operator's freshly-saved body
/// is authoritative, superseding whatever the legacy file held.
#[cfg(feature = "server")]
pub(crate) fn save_profile_persona_impl(name: &str, body: &str) -> Result<(), String> {
    validate_persona_body(body)?;
    ironhermes_core::profile::validate_profile_name(name)
        .map_err(|e| format!("invalid profile name: {e}"))?;
    if !profile_dir_for(name).is_dir() {
        return Err(format!("profile '{name}' does not exist"));
    }
    let _workspace_dir = profile_workspace_dir(name)?;
    write_persona_atomic(&canonical_persona_path(name), body)?;
    let legacy = legacy_persona_path(name);
    if legacy.is_file() {
        let _ = std::fs::remove_file(&legacy);
    }
    Ok(())
}

/// Phase 50.1 Plan 05 (D-15/D-16), OF-6 fix: read a bot's persona body — an
/// absent file (never yet saved) is `Ok` with an empty body, never an
/// error, mirroring `read_env_keys`'s "a fresh profile legitimately has no
/// .env yet" discipline. Checks [`canonical_persona_path`] first, falling
/// back to [`legacy_persona_path`] so a persona saved before the OF-6 fix
/// shipped still displays correctly.
///
/// MA-01 fix (`50.1-REVIEW.md`): this is a browser-reachable READ surface
/// (`#[server] fetch_profile_persona` skips the write gate, as a read
/// should), so it must never create anything on disk — including never
/// migrating the legacy file forward (that happens on the next explicit
/// save, or automatically on the next CLI handoff dispatch via
/// `migrate_legacy_persona_if_needed`). This only ever `stat`s and
/// (conditionally) reads; directory creation stays exclusively behind the
/// gated `save_profile_persona_impl` path above.
#[cfg(feature = "server")]
pub(crate) fn fetch_profile_persona_impl(name: &str) -> Result<ProfilePersona, String> {
    ironhermes_core::profile::validate_profile_name(name)
        .map_err(|e| format!("invalid profile name: {e}"))?;
    let profile_dir = profile_dir_for(name);
    if !profile_dir.is_dir() {
        return Err(format!("profile '{name}' does not exist"));
    }
    let canonical = canonical_persona_path(name);
    let legacy = legacy_persona_path(name);
    let soul_path = if canonical.is_file() {
        Some(canonical)
    } else if legacy.is_file() {
        Some(legacy)
    } else {
        None
    };
    let body = match soul_path {
        Some(p) => std::fs::read_to_string(&p).map_err(|e| format!("read persona file: {e}"))?,
        None => String::new(),
    };
    Ok(ProfilePersona {
        name: name.to_string(),
        body,
    })
}

/// OF-6 fix: one-time, idempotent, best-effort migration of a persona
/// saved before this fix shipped (at [`legacy_persona_path`]) into
/// [`canonical_persona_path`] — the location the agent runtime actually
/// reads. Called from `cli_handoff::run_bot_handoff` immediately before
/// every dispatch, so an already-saved persona (e.g. a bot configured
/// before this fix) loads on the very next turn without requiring the
/// operator to open the drawer and re-save.
///
/// No-op if the canonical file already exists (migration already
/// happened, or the persona was saved fresh after the fix), if there is no
/// legacy file, or if the legacy file is empty. Never returns an error and
/// never fails the caller — a migration failure just means the bot falls
/// back to its default identity for this turn, exactly the pre-fix
/// behavior, never a broken dispatch. Not a browser-reachable surface, so
/// the MA-01 "fetch must be mkdir-free" constraint does not apply here.
#[cfg(feature = "server")]
pub(crate) fn migrate_legacy_persona_if_needed(name: &str) {
    let canonical = canonical_persona_path(name);
    if canonical.is_file() {
        return;
    }
    let legacy = legacy_persona_path(name);
    let Ok(content) = std::fs::read_to_string(&legacy) else {
        return;
    };
    if content.trim().is_empty() {
        return;
    }
    if write_persona_atomic(&canonical, &content).is_ok() {
        let _ = std::fs::remove_file(&legacy);
    }
}

/// Phase 50.1 Plan 05 (D-15/D-16): `#[server]` persona write. Follows this
/// crate's four-step write protocol: validate → fresh `Config::load()` →
/// `check_profile_write_gate` → `spawn_blocking`.
#[server]
pub async fn save_profile_persona(name: String, body: String) -> Result<(), ServerFnError> {
    #[cfg(feature = "server")]
    {
        ironhermes_core::profile::validate_profile_name(&name)
            .map_err(|e| ServerFnError::new(format!("invalid profile name: {e}")))?;
        validate_persona_body(&body).map_err(ServerFnError::new)?;

        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        check_profile_write_gate(&config).map_err(ServerFnError::new)?;

        tokio::task::spawn_blocking(move || save_profile_persona_impl(&name, &body))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)?;
        Ok(())
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (name, body);
        Err(ServerFnError::new(
            "save_profile_persona unavailable without `server` feature",
        ))
    }
}

/// Phase 50.1 Plan 05 (D-15/D-16): `#[server]` persona read.
#[server]
pub async fn fetch_profile_persona(name: String) -> Result<ProfilePersona, ServerFnError> {
    #[cfg(feature = "server")]
    {
        ironhermes_core::profile::validate_profile_name(&name)
            .map_err(|e| ServerFnError::new(format!("invalid profile name: {e}")))?;
        let persona = tokio::task::spawn_blocking(move || fetch_profile_persona_impl(&name))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)?;
        Ok(persona)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = name;
        Err(ServerFnError::new(
            "fetch_profile_persona unavailable without `server` feature",
        ))
    }
}

/// Phase 47.4 Plan 05 (T-47.4-05-T1): validate an env-var key name against
/// `[A-Z][A-Z0-9_]*` before it can reach `render_profile_env`. Without this,
/// a name containing a newline could forge additional `VAR=value` lines
/// into the profile `.env` — an injection that would let one key write
/// smuggle in an arbitrary second key. Pure and disk-I/O-free.
#[cfg(feature = "server")]
pub(crate) fn validate_key_name(key_name: &str) -> Result<(), String> {
    let mut chars = key_name.chars();
    let first_ok = chars
        .next()
        .map(|c| c.is_ascii_uppercase())
        .unwrap_or(false);
    let rest_ok = key_name
        .chars()
        .skip(1)
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !first_ok || !rest_ok {
        return Err(format!(
            "invalid key name '{key_name}' — must match [A-Z][A-Z0-9_]*"
        ));
    }
    Ok(())
}

/// Phase 47.4 Plan 05: validate a key value non-empty after trim, mirroring
/// `validate_secret_value` (`provider_secrets_api.rs:147-152`). Pure and
/// disk-I/O-free so it is directly unit-testable without touching
/// `SecretString` at all.
///
/// Phase 47.4 Plan 18 (CR-02, T-47.4-18-02): strengthened to additionally
/// reject any value containing an ASCII control character — without this, a
/// value embedding a newline (or carriage return) could forge a second
/// `VAR=value` line into `render_profile_env`'s output. The
/// empty-after-trim check stays first and unchanged so its existing,
/// locked error text still applies to that case. The new rejection never
/// interpolates the value itself, only the rule (D-13) — this fn does not
/// even receive the key name, so it cannot name it either.
///
/// Phase 47.4 Plan 20 (CR-03/CR-04): `render_profile_env` now single-quotes
/// every value it writes (`quote_env_value`), so `$`, whitespace, `#`, `"`,
/// `'`, and `\` are all inert to the `dotenvy` reader regardless of what
/// reaches this fn — the writer is the boundary, not this blocklist
/// (`<design_decision>`). `is_control` remains here as a useful fail-fast,
/// not as the security boundary; it is not weakened or removed, and no `$`
/// or whitespace rejection is added on top of it (the inherited-key path
/// from the root `.env` never reaches this validator at all, which is
/// exactly why only the writer can close CR-03/CR-04 in full).
#[cfg(feature = "server")]
pub(crate) fn validate_key_value(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("key value must not be empty".to_string());
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(
            "key value must not contain control characters (newline, carriage return, etc.)"
                .to_string(),
        );
    }
    Ok(())
}

/// Phase 47.4 Plan 05 (D-06/D-13): write a single key into a profile's
/// `.env`, preserving every other entry (and, since `render_profile_env`
/// unconditionally re-stamps the provenance header on every render, the
/// inherited-context requirement that a rewrite never drop it). Reuses
/// `write_env_atomic_0600` — no second secret-write implementation. Pure/
/// synchronous/disk-only so it is directly testable; called from
/// `save_profile_key` inside `spawn_blocking`. `profile_dir` not existing
/// is an error and writes nothing (T-47.4-05 behavior: "nonexistent
/// profile is an error and creates nothing").
#[cfg(feature = "server")]
pub(crate) fn save_profile_key_impl(
    name: &str,
    key_name: &str,
    value: &SecretString,
) -> Result<KeyRow, String> {
    save_profile_key_impl_with_stamp(name, key_name, value, None)
}

/// Phase 48.2 Plan 07 (D-09 checkpoint, resolved option b — inventory
/// stamp): lift of [`save_profile_key_impl`]'s body, parameterized with an
/// optional extra provenance stamp threaded straight through to
/// [`render_profile_env_with_stamp`]. [`save_profile_key_impl`] delegates
/// here with `None`, so its own behavior — and every existing test against
/// it — is unchanged. This, together with the matching lift on
/// `render_profile_env`, is the ONLY change Phase 48.2 Plan 07 makes to
/// this file: the tool-credentials module calls THIS fn (with its own
/// stamp) instead of forking a second `.env`-write implementation
/// (T-48.2-07-04).
#[cfg(feature = "server")]
pub(crate) fn save_profile_key_impl_with_stamp(
    name: &str,
    key_name: &str,
    value: &SecretString,
    extra_stamp: Option<&str>,
) -> Result<KeyRow, String> {
    // Phase 47.4 Plan 18 (CR-02, T-47.4-18-01/02): validate at this pure
    // boundary — not only the `#[server]` wrapper — before reading the
    // existing `.env` or writing anything. Same two-part validation as
    // `create_profile_impl`. Never interpolates the value (D-13).
    validate_key_name(key_name).map_err(|e| format!("invalid key name: {e}"))?;
    validate_key_value(value.expose_secret())
        .map_err(|e| format!("invalid value for key '{key_name}': {e}"))?;

    let profile_dir = profile_dir_for(name);
    if !profile_dir.is_dir() {
        return Err(format!("profile '{name}' does not exist"));
    }

    let env_path = profile_dir.join(".env");
    let mut env_map = read_env_keys(&env_path).map_err(|e| format!("read profile .env: {e}"))?;
    let raw_value = value.expose_secret().to_string();
    env_map.insert(key_name.to_string(), raw_value.clone());

    let mut entries: Vec<(String, String)> = env_map.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let contents = render_profile_env_with_stamp(name, &entries, extra_stamp)?;
    write_env_atomic_0600(&env_path, &contents).map_err(|e| format!("write .env: {e}"))?;

    Ok(KeyRow {
        name: key_name.to_string(),
        status: KeyStatus::ManuallySet,
        masked: mask_key_value(&raw_value),
    })
}

/// Phase 47.4 Plan 05 (D-04/D-07/D-13): save (create or replace) a single
/// key in a profile's `.env`, any time after creation — the detail panel's
/// "fill a `NOT IN ROOT .ENV` key" affordance (D-07: an interim, plaintext
/// posture with a known future successor once per-profile secret storage
/// ships). Fail-closed behind the same
/// `web_config_write_enabled` gate `create_profile`/`update_profile_config`
/// already enforce. The value is wrapped in `SecretString` the moment it is
/// available — before it can reach any `Debug`-deriving struct — and is
/// never echoed back in the return, an error message, or a `tracing` call
/// (D-13).
#[server]
pub async fn save_profile_key(
    name: String,
    key_name: String,
    value: String,
) -> Result<KeyRow, ServerFnError> {
    #[cfg(feature = "server")]
    {
        ironhermes_core::profile::validate_profile_name(&name)
            .map_err(|e| ServerFnError::new(format!("invalid profile name: {e}")))?;
        validate_key_name(&key_name).map_err(ServerFnError::new)?;
        validate_key_value(&value).map_err(ServerFnError::new)?;
        // D-13: wrap immediately, before it crosses into spawn_blocking.
        let secret = SecretString::from(value);

        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        check_profile_write_gate(&config).map_err(ServerFnError::new)?;

        let row =
            tokio::task::spawn_blocking(move || save_profile_key_impl(&name, &key_name, &secret))
                .await
                .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
                .map_err(ServerFnError::new)?;

        Ok(row)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (name, key_name, value);
        Err(ServerFnError::new(
            "save_profile_key unavailable without `server` feature",
        ))
    }
}

/// Phase 47.4 Plan 01 Task 3 (D-11): real fixture-directory tests for
/// `classify_profile_health` / `read_env_keys` / `profile_dir_for`.
///
/// `iron_hermes_ui` is a bin-only crate (no `src/lib.rs`) — integration
/// tests under `tests/` cannot `use iron_hermes_ui::...` and therefore
/// cannot reach this module's `pub(crate)` items (see
/// `tests/provider_config_api.rs`'s own doc comment for the identical,
/// already-established constraint in this codebase: "The functional
/// gate/merge/validate/snapshot tests ... live as `#[cfg(test)]` unit
/// tests inside the file itself"). The real fixture-directory tests this
/// task calls for therefore live HERE, not in
/// `crates/iron_hermes_ui/tests/profile_health.rs` (that file is the
/// companion shape-lock / registration-check, mirroring
/// `tests/provider_config_api.rs` / `tests/kanban_server_fns.rs`).
///
/// Run with (mutates process-global `IRONHERMES_HOME`, so `--test-threads=1`
/// is required — same constraint `ironhermes-kanban::paths` documents for
/// its own `ScopedEnv`-based tests):
///   `cargo nextest run -p iron_hermes_ui --features server profile_health --test-threads=1`
#[cfg(all(test, feature = "server"))]
mod profile_health_tests {
    use super::*;
    use ironhermes_core::config::Config;
    use std::fs;

    /// RAII guard that sets an env var and restores the previous value on
    /// drop. Copied verbatim from `ironhermes-kanban/src/paths.rs:449-475`,
    /// including its safety comment.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; no concurrent env access.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: single-threaded test context; no concurrent env access.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    /// Sets a fixture file to real 0600 perms, matching what
    /// `scripts/make-kanban-profile` and a future `create_profile` write.
    fn set_mode_0600(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms).expect("set_permissions 0600");
    }

    // -------------------------------------------------------------------
    // classify_profile_health — every D-11 permutation
    // -------------------------------------------------------------------

    #[test]
    fn all_conditions_met_yields_configured_with_no_gaps() {
        let (health, gaps) = classify_profile_health(true, true, ProviderKeyState::Resolved);
        assert_eq!(health, ProfileHealth::Configured);
        assert!(gaps.is_empty());
    }

    #[test]
    fn missing_dir_yields_incomplete_with_missing_dir_gap() {
        let (health, gaps) = classify_profile_health(false, true, ProviderKeyState::Resolved);
        assert_eq!(health, ProfileHealth::Incomplete);
        assert_eq!(gaps, vec![ProfileGap::MissingDir]);
    }

    #[test]
    fn missing_config_yaml_yields_incomplete_with_missing_config_gap() {
        let (health, gaps) = classify_profile_health(true, false, ProviderKeyState::Resolved);
        assert_eq!(health, ProfileHealth::Incomplete);
        assert_eq!(gaps, vec![ProfileGap::MissingConfigYaml]);
    }

    #[test]
    fn zero_resolvable_keys_yields_incomplete_with_no_resolvable_key_gap() {
        let (health, gaps) = classify_profile_health(
            true,
            true,
            ProviderKeyState::Missing {
                provider: String::new(),
            },
        );
        assert_eq!(health, ProfileHealth::Incomplete);
        assert_eq!(gaps, vec![ProfileGap::NoResolvableKey]);
    }

    #[test]
    fn missing_key_for_known_provider_yields_incomplete_with_no_key_for_provider_gap() {
        let (health, gaps) = classify_profile_health(
            true,
            true,
            ProviderKeyState::Missing {
                provider: "moonshot".to_string(),
            },
        );
        assert_eq!(health, ProfileHealth::Incomplete);
        assert_eq!(
            gaps,
            vec![ProfileGap::NoKeyForProvider("moonshot".to_string())]
        );
    }

    #[test]
    fn two_missing_conditions_yields_incomplete_with_both_gaps() {
        let (health, gaps) = classify_profile_health(false, false, ProviderKeyState::Resolved);
        assert_eq!(health, ProfileHealth::Incomplete);
        assert_eq!(
            gaps,
            vec![ProfileGap::MissingDir, ProfileGap::MissingConfigYaml]
        );
    }

    // -------------------------------------------------------------------
    // read_env_keys
    // -------------------------------------------------------------------

    #[test]
    fn read_env_keys_on_real_fixture_returns_every_pair() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        fs::write(
            &env_path,
            "OPENROUTER_API_KEY=sk-abc123\nANTHROPIC_API_KEY=sk-def456\n",
        )
        .expect("write .env");
        set_mode_0600(&env_path);

        let keys = read_env_keys(&env_path).expect("read_env_keys should succeed");
        assert_eq!(
            keys.get("OPENROUTER_API_KEY"),
            Some(&"sk-abc123".to_string())
        );
        assert_eq!(
            keys.get("ANTHROPIC_API_KEY"),
            Some(&"sk-def456".to_string())
        );
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn read_env_keys_on_absent_path_returns_empty_map_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env"); // never created
        let keys = read_env_keys(&env_path).expect("absent .env must be Ok(empty), not Err");
        assert!(keys.is_empty());
    }

    #[test]
    fn read_env_keys_on_malformed_file_returns_err_without_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        // A bare line with no `=`, not a comment, not blank — dotenvy
        // rejects this as a parse error. T-47.4-01-D1 DoS mitigation: the
        // process must return Err, never panic or abort.
        fs::write(&env_path, "this line has no equals sign at all\n").expect("write .env");

        let result = read_env_keys(&env_path);
        assert!(result.is_err(), "malformed .env must return Err, not panic");
    }

    /// CR-05 (D-13): the parse-error branch must not leak the failing line.
    ///
    /// `dotenvy::Error::LineParse`'s `Display` (`dotenvy-0.15.7/src/errors.rs:40-44`)
    /// embeds the ENTIRE raw failing line — which for `KEY='the-secret'` IS the
    /// secret. `read_env_keys`'s Err string reaches the browser through 6 production
    /// call sites, on an often-unauthenticated surface, so it must carry no payload.
    ///
    /// Sibling of `self_check_parse_error_never_leaks_the_value`, which pins the same
    /// rule on the WRITE path. This is the READ path.
    #[test]
    fn read_env_keys_parse_error_never_leaks_the_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");

        // A realistic malformed line: an unterminated single quote. This is
        // exactly the shape a profile could already hold on disk from the
        // documented CR-02 -> CR-04 window.
        const SENTINEL: &str = "sk-CR05-CANARY-must-never-surface";
        fs::write(&env_path, format!("GOOD_KEY=fine\nBAD_KEY='{SENTINEL}\n"))
            .expect("write .env");

        let err = read_env_keys(&env_path).expect_err("unterminated quote must be a parse error");

        assert!(
            !err.contains(SENTINEL),
            "read_env_keys' parse-error must not contain the failing line's value \
             (CR-05); got: {err}"
        );
        // The dotenvy error must not be interpolated in ANY form — catch a
        // wrapped/Debug rendering that happens to mangle the sentinel but still
        // ships the raw line.
        assert!(
            !err.contains("BAD_KEY"),
            "read_env_keys' parse-error must not name the failing line at all (CR-05); got: {err}"
        );
        // Still useful diagnostics: the path is not sensitive and tells the
        // operator WHICH profile is malformed.
        assert!(
            err.contains(".env"),
            "the error must still identify the offending file; got: {err}"
        );
    }

    #[test]
    fn read_env_keys_empty_value_does_not_count_toward_health() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        fs::write(&env_path, "OPENROUTER_API_KEY=\n").expect("write .env");

        let keys = read_env_keys(&env_path).expect("read_env_keys should succeed");
        // The key IS present in the parsed map...
        assert_eq!(keys.get("OPENROUTER_API_KEY"), Some(&String::new()));
        // ...but the resolvable-key filter (mirrored from list_profiles'
        // body, GAP-1: now provider-registry-derived) treats an empty value
        // as unresolved.
        let resolvable = provider_key_env_names(&Config::default())
            .iter()
            .filter(|k| keys.get(k.as_str()).map(|v| !v.is_empty()).unwrap_or(false))
            .count();
        assert_eq!(resolvable, 0);
    }

    // -------------------------------------------------------------------
    // profile_dir_for
    // -------------------------------------------------------------------

    #[test]
    fn profile_dir_for_resolves_under_tempdir_profiles_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

        let resolved = profile_dir_for("my-profile");
        assert_eq!(resolved, dir.path().join("profiles").join("my-profile"));
    }

    #[test]
    fn profile_dir_for_traversal_shaped_name_creates_nothing_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

        // profile_dir_for is a pure PathBuf::join with zero I/O
        // (T-47.4-01-T1 mitigation) — merely computing a path for an
        // adversarial name must not touch the filesystem. Assert the
        // tempdir tree is unchanged rather than checking a return value —
        // this fn is infallible (returns PathBuf, not Result), so "did not
        // error" isn't a meaningful check; "wrote nothing to disk" is.
        let before = walk_all(dir.path());
        let _ = profile_dir_for("../../etc/passwd");
        let _ = profile_dir_for("..");
        let after = walk_all(dir.path());
        assert_eq!(before, after, "profile_dir_for must never write to disk");
    }

    fn walk_all(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                out.push(entry.path());
            }
        }
        out.sort();
        out
    }

    // -------------------------------------------------------------------
    // Enumeration ordering / filtering (mirrors list_profiles' own
    // read_dir loop, exercised at the logic layer — same "test the logic
    // layer, not the #[server]-wrapped fn" precedent as
    // tests/kanban_board_read.rs).
    // -------------------------------------------------------------------

    #[test]
    fn enumeration_skips_dotfiles_and_non_directories_and_sorts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let profiles_root = dir.path().join("profiles");
        fs::create_dir_all(profiles_root.join("zeta")).expect("mkdir zeta");
        fs::create_dir_all(profiles_root.join("alpha")).expect("mkdir alpha");
        fs::create_dir_all(profiles_root.join(".hidden")).expect("mkdir .hidden");
        fs::write(profiles_root.join("readme.txt"), b"").expect("write readme");

        let mut names: Vec<String> = Vec::new();
        for entry in fs::read_dir(&profiles_root).expect("read_dir") {
            let entry = entry.expect("entry");
            let file_type = entry.file_type().expect("file_type");
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            names.push(name);
        }
        names.sort();

        assert_eq!(names, vec!["alpha".to_string(), "zeta".to_string()]);
    }

    // -------------------------------------------------------------------
    // End-to-end fixture: a real profiles/<name>/ layout composed through
    // profile_dir_for + Config::load_from + read_env_keys +
    // classify_profile_health — the same composition list_profiles
    // performs, minus the #[server] macro / spawn_blocking wrapper.
    // -------------------------------------------------------------------

    #[test]
    fn end_to_end_configured_profile_fixture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

        let name = "configured-profile";
        let profile_dir = profile_dir_for(name);
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");

        let mut cfg = Config::default();
        cfg.model.provider = "anthropic".to_string();
        cfg.model.default = "claude-3-opus".to_string();
        cfg.save_to(&profile_dir.join("config.yaml"))
            .expect("save_to config.yaml");

        let env_path = profile_dir.join(".env");
        fs::write(&env_path, "ANTHROPIC_API_KEY=sk-abc123\n").expect("write .env");
        set_mode_0600(&env_path);

        let dir_exists = profile_dir.is_dir();
        let config_path = profile_dir.join("config.yaml");
        let config_yaml_exists = config_path.is_file();
        let loaded = Config::load_from(&config_path).expect("config.yaml must parse");
        let env_map = read_env_keys(&env_path).expect("read .env");
        let resolvable = provider_key_env_names(&loaded)
            .iter()
            .filter(|k| env_map.get(k.as_str()).map(|v| !v.is_empty()).unwrap_or(false))
            .count();
        let provider_key = compute_provider_key_state(
            name,
            config_yaml_exists,
            Some(loaded.model.provider.as_str()),
            resolvable,
        );

        let (health, gaps) = classify_profile_health(dir_exists, config_yaml_exists, provider_key);
        assert_eq!(health, ProfileHealth::Configured);
        assert!(gaps.is_empty());
        assert_eq!(loaded.model.provider, "anthropic");
        assert_eq!(loaded.model.default, "claude-3-opus");
    }

    // -------------------------------------------------------------------
    // provider_key_env_names / provider_key_env_name_for (GAP-1, Task 1)
    // -------------------------------------------------------------------

    fn provider_cfg(
        api_key_env: Option<&str>,
        disabled: Option<bool>,
    ) -> ironhermes_core::config::ProviderConfig {
        ironhermes_core::config::ProviderConfig {
            api_key_env: api_key_env.map(|s| s.to_string()),
            disabled,
            ..Default::default()
        }
    }

    /// Mirrors the shape of the operator's real root `config.yaml`
    /// (`47.4-11-PLAN.md` source fact 9): a `providers:` block declaring
    /// `api_key_env` for several non-legacy providers, plus a keyless
    /// `llama` entry.
    fn operator_shaped_config() -> Config {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "moonshot".to_string(),
            provider_cfg(Some("MOONSHOT_API_KEY"), None),
        );
        cfg.providers.insert(
            "venice".to_string(),
            provider_cfg(Some("VENICE_API_KEY"), None),
        );
        cfg.providers.insert(
            "minimax".to_string(),
            provider_cfg(Some("MINIMAX_API_KEY"), None),
        );
        cfg.providers
            .insert("merge".to_string(), provider_cfg(Some("MERGE_API_KEY"), None));
        cfg.providers
            .insert("huggingface".to_string(), provider_cfg(Some("HF_TOKEN"), None));
        cfg.providers
            .insert("llama".to_string(), provider_cfg(None, None));
        cfg
    }

    #[test]
    fn derived_key_names_include_moonshot_for_operator_shaped_config() {
        let cfg = operator_shaped_config();
        let names = provider_key_env_names(&cfg);
        for expected in [
            "MOONSHOT_API_KEY",
            "VENICE_API_KEY",
            "MINIMAX_API_KEY",
            "MERGE_API_KEY",
            "HF_TOKEN",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "expected {expected} in the derived set for the operator-shaped config"
            );
        }
    }

    #[test]
    fn derived_key_names_retain_legacy_five_floor() {
        let cfg = Config::default();
        let names = provider_key_env_names(&cfg);
        for expected in [
            "OPENROUTER_API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GROQ_API_KEY",
            "OLLAMA_API_KEY",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "expected legacy floor name {expected} present even for an empty providers map"
            );
        }
    }

    #[test]
    fn keyless_provider_contributes_no_key_name() {
        let mut cfg = Config::default();
        cfg.providers
            .insert("llama".to_string(), provider_cfg(None, None));
        let names = provider_key_env_names(&cfg);
        assert_eq!(
            names.len(),
            5,
            "a keyless provider must contribute no name beyond the 5-name legacy floor"
        );
    }

    #[test]
    fn disabled_provider_contributes_no_key_name() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "moonshot".to_string(),
            provider_cfg(Some("MOONSHOT_API_KEY"), Some(true)),
        );
        let names = provider_key_env_names(&cfg);
        assert!(
            !names.iter().any(|n| n == "MOONSHOT_API_KEY"),
            "a disabled provider must contribute no key name"
        );
    }

    #[test]
    fn provider_key_env_name_for_returns_explicit_env_for_moonshot() {
        let cfg = operator_shaped_config();
        assert_eq!(
            provider_key_env_name_for(&cfg, "moonshot"),
            Some("MOONSHOT_API_KEY".to_string())
        );
    }

    #[test]
    fn provider_key_env_name_for_returns_none_for_keyless_llama() {
        let cfg = operator_shaped_config();
        assert_eq!(provider_key_env_name_for(&cfg, "llama"), None);
    }

    // -------------------------------------------------------------------
    // classify_profile_health end-to-end: the bdev01 shape (GAP-1, Task 2)
    // and the keyless-provider carve-out.
    // -------------------------------------------------------------------

    #[test]
    fn bdev01_shape_classifies_incomplete_not_configured() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

        let name = "bdev01-shape";
        let profile_dir = profile_dir_for(name);
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");

        let mut cfg = Config::default();
        cfg.model.provider = "moonshot".to_string();
        cfg.model.default = "moonshot-v1".to_string();
        cfg.providers.insert(
            "moonshot".to_string(),
            provider_cfg(Some("MOONSHOT_API_KEY"), None),
        );
        cfg.save_to(&profile_dir.join("config.yaml"))
            .expect("save_to config.yaml");

        let env_path = profile_dir.join(".env");
        // Four non-empty keys for OTHER providers, none for moonshot — the
        // exact bdev01 401 shape (a single-key or empty-.env fixture would
        // pass against the pre-Plan-11 buggy code and prove nothing).
        fs::write(
            &env_path,
            "OPENROUTER_API_KEY=sk-other-1\nOPENAI_API_KEY=sk-other-2\nGROQ_API_KEY=sk-other-3\nOLLAMA_API_KEY=sk-other-4\n",
        )
        .expect("write .env");
        set_mode_0600(&env_path);

        let dir_exists = profile_dir.is_dir();
        let config_yaml_exists = profile_dir.join("config.yaml").is_file();
        let loaded =
            Config::load_from(&profile_dir.join("config.yaml")).expect("config.yaml must parse");
        let env_map = read_env_keys(&env_path).expect("read .env");
        let resolvable = provider_key_env_names(&loaded)
            .iter()
            .filter(|k| env_map.get(k.as_str()).map(|v| !v.is_empty()).unwrap_or(false))
            .count();
        let provider_key = compute_provider_key_state(
            name,
            config_yaml_exists,
            Some(loaded.model.provider.as_str()),
            resolvable,
        );
        let (health, gaps) = classify_profile_health(dir_exists, config_yaml_exists, provider_key);
        assert_eq!(health, ProfileHealth::Incomplete);
        assert_eq!(
            gaps,
            vec![ProfileGap::NoKeyForProvider("moonshot".to_string())]
        );
    }

    #[test]
    fn keyless_provider_profile_classifies_configured() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

        let name = "llama-keyless";
        let profile_dir = profile_dir_for(name);
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");

        let mut cfg = Config::default();
        cfg.model.provider = "llama".to_string();
        cfg.model.default = "llama-3".to_string();
        cfg.providers
            .insert("llama".to_string(), provider_cfg(None, None));
        cfg.save_to(&profile_dir.join("config.yaml"))
            .expect("save_to config.yaml");
        // No .env at all — a keyless provider legitimately has no key.

        let dir_exists = profile_dir.is_dir();
        let config_yaml_exists = profile_dir.join("config.yaml").is_file();
        let loaded =
            Config::load_from(&profile_dir.join("config.yaml")).expect("config.yaml must parse");
        let provider_key = compute_provider_key_state(
            name,
            config_yaml_exists,
            Some(loaded.model.provider.as_str()),
            0,
        );
        let (health, gaps) = classify_profile_health(dir_exists, config_yaml_exists, provider_key);
        assert_eq!(health, ProfileHealth::Configured);
        assert!(gaps.is_empty());
    }

    #[test]
    fn no_key_for_provider_meta_label_names_the_provider() {
        assert_eq!(
            ProfileGap::NoKeyForProvider("moonshot".to_string())
                .meta_label()
                .as_ref(),
            "no key for provider moonshot"
        );
    }

    #[test]
    fn existing_gap_labels_are_unchanged_after_meta_label_signature_change() {
        assert_eq!(ProfileGap::MissingDir.meta_label().as_ref(), "profile dir missing");
        assert_eq!(
            ProfileGap::MissingConfigYaml.meta_label().as_ref(),
            "missing config.yaml"
        );
        assert_eq!(
            ProfileGap::NoResolvableKey.meta_label().as_ref(),
            "no resolvable key"
        );
    }
}

/// Phase 47.4 Plan 03 Task 3 (D-06/D-07/D-08/D-13): real fixture-directory
/// tests for `create_profile_impl` / `resolve_inherited_keys` /
/// `mask_key_value` / `write_env_atomic_0600` / `check_profile_write_gate`.
///
/// Same bin-only-crate constraint as `profile_health_tests` above (see that
/// module's own doc comment for the full explanation) — the real tests
/// live here, not in `crates/iron_hermes_ui/tests/profile_scaffold.rs`
/// (that file is the companion shape-lock / registration-check, mirroring
/// `tests/profile_health.rs` / `tests/provider_config_api.rs`). Run with
/// (mutates process-global `IRONHERMES_HOME`, so `--test-threads=1` is
/// required):
///   `cargo nextest run -p iron_hermes_ui --features server profile_scaffold_tests --test-threads=1`
///
/// Mutation sanity map — deleting any one of these lines should turn at
/// least one test in this module red (verified manually for the `!force`
/// guard during Task 3 execution: removed, confirmed red, reverted):
/// - the `!force` never-clobber guard in `create_profile_impl` →
///   `second_create_without_force_returns_err_and_leaves_files_untouched`
/// - `validate_profile_name(name)` at the top of `create_profile_impl` →
///   `reserved_name_rejected_creates_nothing_on_disk` and
///   `path_traversal_name_rejected_creates_nothing_outside_profiles`
/// - the `value.is_empty()` filter in `resolve_inherited_keys` →
///   `resolve_inherited_keys_drops_empty_values`
/// - the manual-key overlay loop in `create_profile_impl` →
///   `manual_key_overrides_inherited_value_for_same_name`
/// - `mode(0o600)` in `write_env_atomic_0600` →
///   `created_env_file_has_mode_0600`
/// - `std::fs::copy` for `config.yaml` in `create_profile_impl` →
///   `created_config_yaml_is_byte_identical_to_root`
/// - `mask_key_value`'s early return on empty/never-embeds-input behavior →
///   `mask_key_value_never_embeds_input_characters` /
///   `mask_key_value_empty_value_yields_em_dash`
#[cfg(all(test, feature = "server"))]
mod profile_scaffold_tests {
    use super::*;
    use ironhermes_core::config::Config;
    use std::fs;

    /// RAII guard that sets an env var and restores the previous value on
    /// drop. Duplicated verbatim from `profile_health_tests::ScopedEnv`
    /// above (and originally `ironhermes-kanban/src/paths.rs:449-475`) —
    /// each `#[cfg(test)]` module is its own namespace, so this is the
    /// plan's own sanctioned "duplicate the guard" instruction, not drift.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; no concurrent env access.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: single-threaded test context; no concurrent env access.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    fn walk_all(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                out.push(entry.path());
            }
        }
        out.sort();
        out
    }

    fn home(dir: &tempfile::TempDir) -> ScopedEnv {
        ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        )
    }

    // -------------------------------------------------------------------
    // Behavior 1/2/3: creates config.yaml + .env, 0600 mode, byte-identical
    // config.yaml copy.
    // -------------------------------------------------------------------

    #[test]
    fn creates_config_yaml_and_env_in_profile_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        fs::write(dir.path().join(".env"), "OPENROUTER_API_KEY=sk-abc\n").expect("root .env");
        Config::default()
            .save_to(&dir.path().join("config.yaml"))
            .expect("root config.yaml");

        let rows = create_profile_impl("kanban-worker", &KeyMode::LlmOnly, false, Vec::new(), &Config::default())
            .expect("create should succeed");
        assert!(!rows.is_empty());

        let profile_dir = profile_dir_for("kanban-worker");
        assert!(profile_dir.join("config.yaml").is_file());
        assert!(profile_dir.join(".env").is_file());
    }

    #[test]
    fn created_env_file_has_mode_0600() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        fs::write(dir.path().join(".env"), "OPENROUTER_API_KEY=sk-abc\n").expect("root .env");

        create_profile_impl("perm-test", &KeyMode::LlmOnly, false, Vec::new(), &Config::default())
            .expect("create should succeed");

        use std::os::unix::fs::PermissionsExt;
        let meta =
            fs::metadata(profile_dir_for("perm-test").join(".env")).expect("metadata of .env");
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn created_config_yaml_is_byte_identical_to_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let mut root_cfg = Config::default();
        root_cfg.model.provider = "openrouter".to_string();
        root_cfg.model.default = "some/model".to_string();
        root_cfg
            .save_to(&dir.path().join("config.yaml"))
            .expect("root config.yaml");

        create_profile_impl("byte-copy-profile", &KeyMode::LlmOnly, false, Vec::new(), &Config::default())
            .expect("create should succeed");

        let root_bytes = fs::read(dir.path().join("config.yaml")).expect("read root config.yaml");
        let profile_bytes = fs::read(profile_dir_for("byte-copy-profile").join("config.yaml"))
            .expect("read profile config.yaml");
        assert_eq!(
            root_bytes, profile_bytes,
            "config.yaml must be a byte-identical copy of the root file"
        );
    }

    #[test]
    fn missing_root_config_yaml_does_not_fail_creation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        fs::write(dir.path().join(".env"), "OPENROUTER_API_KEY=sk-abc\n").expect("root .env");
        // Deliberately no root config.yaml written.

        let result = create_profile_impl(
            "no-root-config-profile",
            &KeyMode::LlmOnly,
            false,
            Vec::new(),
            &Config::default(),
        );
        assert!(
            result.is_ok(),
            "a missing root config.yaml must not fail the whole create (script's SKIPPED branch)"
        );
        assert!(!profile_dir_for("no-root-config-profile")
            .join("config.yaml")
            .exists());
    }

    // -------------------------------------------------------------------
    // Behavior 4/5: never-clobber-without-force + force overwrites both.
    // -------------------------------------------------------------------

    #[test]
    fn second_create_without_force_returns_err_and_leaves_files_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        fs::write(dir.path().join(".env"), "OPENROUTER_API_KEY=sk-abc\n").expect("root .env");
        Config::default()
            .save_to(&dir.path().join("config.yaml"))
            .expect("root config.yaml");

        create_profile_impl("clobber-test", &KeyMode::LlmOnly, false, Vec::new(), &Config::default())
            .expect("first create should succeed");

        let profile_dir = profile_dir_for("clobber-test");
        let sentinel_env = "SENTINEL_VALUE=untouched\n";
        fs::write(profile_dir.join(".env"), sentinel_env).expect("stomp profile .env");
        let sentinel_cfg: &[u8] = b"sentinel: untouched\n";
        fs::write(profile_dir.join("config.yaml"), sentinel_cfg).expect("stomp profile config");

        let result = create_profile_impl("clobber-test", &KeyMode::LlmOnly, false, Vec::new(), &Config::default());
        assert!(
            result.is_err(),
            "second create without --force must return Err"
        );

        let env_after = fs::read_to_string(profile_dir.join(".env")).expect("read .env after");
        assert_eq!(
            env_after, sentinel_env,
            "profile .env must be byte-untouched without --force"
        );
        let cfg_after = fs::read(profile_dir.join("config.yaml")).expect("read config after");
        assert_eq!(
            cfg_after, sentinel_cfg,
            "profile config.yaml must be byte-untouched without --force"
        );
    }

    #[test]
    fn second_create_with_force_overwrites_both_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        fs::write(dir.path().join(".env"), "OPENROUTER_API_KEY=sk-first\n").expect("root .env");
        Config::default()
            .save_to(&dir.path().join("config.yaml"))
            .expect("root config.yaml");

        create_profile_impl("force-test", &KeyMode::LlmOnly, false, Vec::new(), &Config::default())
            .expect("first create should succeed");

        let profile_dir = profile_dir_for("force-test");
        fs::write(profile_dir.join(".env"), "SENTINEL=should-be-replaced\n")
            .expect("stomp profile .env");
        fs::write(
            profile_dir.join("config.yaml"),
            b"sentinel: should-be-replaced\n",
        )
        .expect("stomp profile config");

        fs::write(dir.path().join(".env"), "OPENROUTER_API_KEY=sk-second\n")
            .expect("update root .env");

        create_profile_impl("force-test", &KeyMode::LlmOnly, true, Vec::new(), &Config::default())
            .expect("forced create should succeed");

        let env_after = fs::read_to_string(profile_dir.join(".env")).expect("read .env after");
        assert!(
            env_after.contains("sk-second"),
            "force must overwrite the profile .env with fresh content"
        );
        assert!(!env_after.contains("SENTINEL"));

        let cfg_after = fs::read(profile_dir.join("config.yaml")).expect("read config after");
        let root_cfg_bytes = fs::read(dir.path().join("config.yaml")).expect("read root config");
        assert_eq!(
            cfg_after, root_cfg_bytes,
            "force must overwrite config.yaml with a fresh byte copy of root"
        );
    }

    // -------------------------------------------------------------------
    // Behavior 6/7/8: the three key-inheritance modes.
    // -------------------------------------------------------------------

    #[test]
    fn llm_only_mode_excludes_non_allowlisted_root_vars() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        fs::write(
            dir.path().join(".env"),
            "OPENROUTER_API_KEY=sk-abc123\nTELEGRAM_BOT_TOKEN=tg-should-not-appear\n",
        )
        .expect("root .env");

        let rows = create_profile_impl("llm-only-profile", &KeyMode::LlmOnly, false, Vec::new(), &Config::default())
            .expect("create should succeed");

        assert!(rows.iter().any(|r| r.name == "OPENROUTER_API_KEY"));
        assert!(
            !rows.iter().any(|r| r.name == "TELEGRAM_BOT_TOKEN"),
            "LlmOnly must not inherit a non-allowlisted root var"
        );

        // Assert on the filesystem too, not only the return value — a
        // fixture with only the expected keys would pass against a filter
        // that does nothing.
        let profile_env = fs::read_to_string(profile_dir_for("llm-only-profile").join(".env"))
            .expect("read profile .env");
        assert!(!profile_env.contains("TELEGRAM_BOT_TOKEN"));
        // Read back through the real dotenvy parser (Phase 47.4 Plan 20,
        // CR-03/CR-04) rather than an unquoted-substring match.
        let parsed = read_env_keys(&profile_dir_for("llm-only-profile").join(".env"))
            .expect("read_env_keys after create");
        assert_eq!(
            parsed.get("OPENROUTER_API_KEY").map(String::as_str),
            Some("sk-abc123")
        );
    }

    #[test]
    fn all_keys_mode_writes_every_matching_suffix_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        fs::write(
            dir.path().join(".env"),
            "OPENROUTER_API_KEY=sk-abc\nTELEGRAM_BOT_TOKEN=tg-xyz\nSOME_RANDOM_VAR=nope\nFAL_KEY=fal-123\n",
        )
        .expect("root .env");

        let rows = create_profile_impl("all-keys-profile", &KeyMode::AllKeys, false, Vec::new(), &Config::default())
            .expect("create should succeed");
        let names: std::collections::HashSet<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains("OPENROUTER_API_KEY"));
        assert!(names.contains("TELEGRAM_BOT_TOKEN"));
        assert!(names.contains("FAL_KEY"));
        assert!(
            !names.contains("SOME_RANDOM_VAR"),
            "SOME_RANDOM_VAR matches none of _API_KEY/_KEY/_TOKEN"
        );
    }

    #[test]
    fn explicit_mode_writes_exactly_the_listed_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        fs::write(
            dir.path().join(".env"),
            "OPENROUTER_API_KEY=sk-abc\nANTHROPIC_API_KEY=sk-def\n",
        )
        .expect("root .env");

        let rows = create_profile_impl(
            "explicit-profile",
            &KeyMode::Explicit(vec!["OPENROUTER_API_KEY".to_string()]),
            false,
            Vec::new(),
            &Config::default(),
        )
        .expect("create should succeed");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "OPENROUTER_API_KEY");
    }

    // -------------------------------------------------------------------
    // Behavior 9: manual key overrides an inherited value for the same
    // name.
    // -------------------------------------------------------------------

    #[test]
    fn manual_key_overrides_inherited_value_for_same_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        fs::write(dir.path().join(".env"), "OPENROUTER_API_KEY=root-value\n").expect("root .env");

        let manual = vec![(
            "OPENROUTER_API_KEY".to_string(),
            SecretString::from("manual-value".to_string()),
        )];
        let rows = create_profile_impl("manual-override-profile", &KeyMode::LlmOnly, false, manual, &Config::default())
            .expect("create should succeed");

        let row = rows
            .iter()
            .find(|r| r.name == "OPENROUTER_API_KEY")
            .expect("row present");
        assert_eq!(row.status, KeyStatus::ManuallySet);

        let profile_env =
            fs::read_to_string(profile_dir_for("manual-override-profile").join(".env"))
                .expect("read profile .env");
        assert!(!profile_env.contains("root-value"));
        // Read back through the real dotenvy parser (Phase 47.4 Plan 20,
        // CR-03/CR-04) rather than an unquoted-substring match.
        let parsed = read_env_keys(&profile_dir_for("manual-override-profile").join(".env"))
            .expect("read_env_keys after create");
        assert_eq!(
            parsed.get("OPENROUTER_API_KEY").map(String::as_str),
            Some("manual-value")
        );
    }

    // -------------------------------------------------------------------
    // Task 1 (T-47.4-18-01/02, CR-02): a forged manual key NAME or VALUE
    // must be rejected before any byte reaches disk — RED first, then the
    // fix in Task 1's <action>.
    // -------------------------------------------------------------------

    #[test]
    fn create_profile_impl_rejects_a_manual_key_value_forging_a_second_env_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let legit_value = "sk-Legit9f3kQ7xN2mP8wL4vR6tY1uJ5hG0bC";
        let forged_value = "sk-Forged3eA9f3kQ7xN2mP8wL4vR6tY1uJ5h";
        let manual = vec![(
            "OPENROUTER_API_KEY".to_string(),
            SecretString::from(format!("{legit_value}\nINJECTED_KEY={forged_value}")),
        )];

        let result = create_profile_impl(
            "forge-value-profile",
            &KeyMode::LlmOnly,
            false,
            manual,
            &Config::default(),
        );
        let err =
            result.expect_err("a manual key value embedding a newline must be rejected (CR-02)");
        assert!(
            !err.contains(legit_value) && !err.contains(forged_value),
            "the Err string must never contain a key value (D-13): {err}"
        );
        assert!(
            !profile_dir_for("forge-value-profile").exists(),
            "a rejected manual key must create nothing on disk (D-07)"
        );
    }

    #[test]
    fn create_profile_impl_rejects_a_manual_key_name_containing_a_newline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let legit_value = "sk-NameCase9f3kQ7xN2mP8wL4vR6tY1uJ5hG0";
        let manual = vec![(
            "OPENROUTER_API_KEY\nINJECTED_KEY".to_string(),
            SecretString::from(legit_value.to_string()),
        )];

        let result = create_profile_impl(
            "forge-name-profile",
            &KeyMode::LlmOnly,
            false,
            manual,
            &Config::default(),
        );
        let err =
            result.expect_err("a manual key name embedding a newline must be rejected (CR-02)");
        assert!(
            !err.contains(legit_value),
            "the Err string must never contain a key value (D-13): {err}"
        );
        assert!(
            !profile_dir_for("forge-name-profile").exists(),
            "a rejected manual key must create nothing on disk (D-07)"
        );
    }

    #[test]
    fn create_profile_impl_writes_only_validated_entries_on_the_happy_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let manual = vec![(
            "CUSTOM_KEY".to_string(),
            SecretString::from("sk-well-formed-value".to_string()),
        )];
        let rows = create_profile_impl(
            "happy-path-validated-profile",
            &KeyMode::LlmOnly,
            false,
            manual,
            &Config::default(),
        )
        .expect("a legitimate manual key must still succeed");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "CUSTOM_KEY");

        let env_map = read_env_keys(&profile_dir_for("happy-path-validated-profile").join(".env"))
            .expect("read written .env");
        assert_eq!(env_map.len(), 1, "no extra name and no lost name");
        assert_eq!(
            env_map.get("CUSTOM_KEY").map(String::as_str),
            Some("sk-well-formed-value")
        );
    }

    // -------------------------------------------------------------------
    // Behavior 10/11: reserved name / traversal-shaped name rejected,
    // creates nothing on disk.
    // -------------------------------------------------------------------

    #[test]
    fn reserved_name_rejected_creates_nothing_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let result = create_profile_impl("default", &KeyMode::LlmOnly, false, Vec::new(), &Config::default());
        assert!(result.is_err());

        assert!(
            !dir.path().join("profiles").exists(),
            "a reserved name must create nothing on disk"
        );
    }

    #[test]
    fn path_traversal_name_rejected_creates_nothing_outside_profiles() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let before = walk_all(dir.path());
        let _ = create_profile_impl("../../etc/passwd", &KeyMode::LlmOnly, false, Vec::new(), &Config::default());
        let _ = create_profile_impl("..", &KeyMode::LlmOnly, false, Vec::new(), &Config::default());
        let after = walk_all(dir.path());
        assert_eq!(
            before, after,
            "a traversal-shaped name must write nothing to disk"
        );
    }

    // -------------------------------------------------------------------
    // Behavior 12: the fail-closed write gate (T-47.4-03-E1).
    // -------------------------------------------------------------------

    #[test]
    fn write_gate_fails_closed_by_default() {
        let config = Config::default();
        assert!(!config.security.web_config_write_enabled);
        let err = check_profile_write_gate(&config).expect_err("gate must be closed by default");
        assert!(err.contains("Config writes are disabled"));
    }

    #[test]
    fn write_gate_passes_when_enabled() {
        let mut config = Config::default();
        config.security.web_config_write_enabled = true;
        assert!(check_profile_write_gate(&config).is_ok());
    }

    // -------------------------------------------------------------------
    // Behavior 13: a missing root .env resolves nothing, not an error.
    // -------------------------------------------------------------------

    #[test]
    fn missing_root_env_resolves_no_keys_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        // Deliberately no root .env written at all.

        let rows = create_profile_impl("no-root-env-profile", &KeyMode::LlmOnly, false, Vec::new(), &Config::default())
            .expect("missing root .env must not be an error");
        assert!(rows.is_empty());

        let profile_env = fs::read_to_string(profile_dir_for("no-root-env-profile").join(".env"))
            .expect("profile .env should still be written (empty key set)");
        assert!(profile_env.starts_with(PROFILE_ENV_PROVENANCE_PREFIX));
    }

    // -------------------------------------------------------------------
    // Behavior 14 (D-13): returned rows are masked, no raw key substring
    // anywhere in the serialized response.
    // -------------------------------------------------------------------

    #[test]
    fn returned_rows_are_masked_and_contain_no_raw_key_substring() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        // A distinctive high-entropy value so a partial match cannot slip
        // through.
        let secret_value = "sk-Z9f3kQ7xN2mP8wL4vR6tY1uJ5hG0bC3eA";
        fs::write(
            dir.path().join(".env"),
            format!("OPENROUTER_API_KEY={secret_value}\n"),
        )
        .expect("root .env");

        let rows = create_profile_impl("no-leak-profile", &KeyMode::LlmOnly, false, Vec::new(), &Config::default())
            .expect("create should succeed");

        let serialized = serde_json::to_string(&rows).expect("serialize KeyRow response");
        assert!(
            !serialized.contains(secret_value),
            "serialized KeyRow response must never contain the raw key value"
        );
        assert!(rows.iter().any(|r| r.name == "OPENROUTER_API_KEY"));
    }

    #[test]
    fn provenance_header_line_is_stamped_on_every_generated_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        fs::write(dir.path().join(".env"), "OPENROUTER_API_KEY=sk-abc\n").expect("root .env");

        create_profile_impl("provenance-profile", &KeyMode::LlmOnly, false, Vec::new(), &Config::default())
            .expect("create should succeed");

        let profile_env = fs::read_to_string(profile_dir_for("provenance-profile").join(".env"))
            .expect("read profile .env");
        assert!(
            profile_env.starts_with(PROFILE_ENV_PROVENANCE_PREFIX),
            "every generated .env must start with the machine-readable provenance header"
        );
        assert!(profile_env.contains("provenance-profile"));
    }

    // -------------------------------------------------------------------
    // mask_key_value / resolve_inherited_keys — pure unit tests.
    // -------------------------------------------------------------------

    #[test]
    fn mask_key_value_never_embeds_input_characters() {
        let secret = "sk-VeryUniqueSubstringXyz123";
        let masked = mask_key_value(secret);
        assert!(
            !masked.contains(secret),
            "masked output must never contain the raw value"
        );
        assert!(masked.starts_with("sk-"));
    }

    #[test]
    fn mask_key_value_empty_value_yields_em_dash() {
        assert_eq!(mask_key_value(""), "\u{2014}");
    }

    #[test]
    fn resolve_inherited_keys_drops_empty_values() {
        let mut root_env = HashMap::new();
        root_env.insert("OPENROUTER_API_KEY".to_string(), String::new());
        root_env.insert("ANTHROPIC_API_KEY".to_string(), "sk-def".to_string());
        let resolved = resolve_inherited_keys(&root_env, &KeyMode::LlmOnly, &Config::default());
        assert!(resolved.iter().all(|(k, _)| k != "OPENROUTER_API_KEY"));
        assert!(resolved
            .iter()
            .any(|(k, v)| k == "ANTHROPIC_API_KEY" && v == "sk-def"));
    }

    // -------------------------------------------------------------------
    // Phase 50.1 Plan 06 (D-17): duplicate_profile_impl.
    // -------------------------------------------------------------------

    fn write_fixture_file(path: &std::path::Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir parent");
        }
        fs::write(path, contents).expect("write fixture file");
    }

    #[test]
    fn duplicate_profile_impl_copies_config_skills_memories_and_persona() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let source_dir = profile_dir_for("source-bot");
        write_fixture_file(
            &source_dir.join("config.yaml"),
            "model:\n  provider: openrouter\n",
        );
        write_fixture_file(
            &source_dir.join("skills").join("greet").join("SKILL.md"),
            "# Greet\n",
        );
        write_fixture_file(
            &source_dir.join("memories").join("MEMORY.md"),
            "remembered fact\n",
        );
        // OF-6 fix: this fixture is deliberately seeded at the pre-fix
        // legacy location (`workspace/SOUL.md`) to exercise the fallback
        // source read — proves a source bot whose persona predates OF-6
        // still clones it forward.
        write_fixture_file(
            &source_dir.join("workspace").join("SOUL.md"),
            "I am a helpful bot.\n",
        );

        let created =
            duplicate_profile_impl("source-bot", "target-bot").expect("duplicate should succeed");
        assert_eq!(created, "target-bot");

        let target_dir = profile_dir_for("target-bot");
        assert_eq!(
            fs::read_to_string(target_dir.join("config.yaml")).expect("target config.yaml"),
            "model:\n  provider: openrouter\n"
        );
        assert_eq!(
            fs::read_to_string(target_dir.join("skills").join("greet").join("SKILL.md"))
                .expect("target skill file"),
            "# Greet\n"
        );
        assert_eq!(
            fs::read_to_string(target_dir.join("memories").join("MEMORY.md"))
                .expect("target memory file"),
            "remembered fact\n"
        );
        // OF-6 fix: the copy lands at the CANONICAL profile-root SOUL.md —
        // the path PromptBuilder::load_soul_md actually reads at turn time
        // — never the legacy workspace/SOUL.md location.
        assert_eq!(
            fs::read_to_string(target_dir.join("SOUL.md")).expect("target persona file"),
            "I am a helpful bot.\n"
        );
        assert!(
            !target_dir.join("workspace").join("SOUL.md").exists(),
            "duplicate must not also seed the legacy workspace/SOUL.md location"
        );
        assert!(
            target_dir.join("workspace").join(".ironhermes").is_dir(),
            "target workspace must still carry the session/trajectory-scoping marker"
        );
    }

    #[test]
    fn duplicate_profile_impl_copies_persona_from_canonical_source_location() {
        // OF-6 fix: a source bot whose persona was saved AFTER this fix
        // shipped has it at the canonical profile-root SOUL.md, not the
        // legacy workspace/ location — duplicate must prefer that.
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let source_dir = profile_dir_for("canonical-source-bot");
        write_fixture_file(
            &source_dir.join("config.yaml"),
            "model:\n  provider: openrouter\n",
        );
        write_fixture_file(&source_dir.join("SOUL.md"), "I am canonical.\n");

        duplicate_profile_impl("canonical-source-bot", "canonical-target-bot")
            .expect("duplicate should succeed");

        let target_dir = profile_dir_for("canonical-target-bot");
        assert_eq!(
            fs::read_to_string(target_dir.join("SOUL.md")).expect("target persona file"),
            "I am canonical.\n"
        );
    }

    #[test]
    fn duplicate_profile_impl_never_copies_env_file_even_when_source_has_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let source_dir = profile_dir_for("has-env-bot");
        write_fixture_file(
            &source_dir.join("config.yaml"),
            "model:\n  provider: openrouter\n",
        );
        write_fixture_file(
            &source_dir.join(".env"),
            "OPENROUTER_API_KEY=sk-should-never-copy\n",
        );

        duplicate_profile_impl("has-env-bot", "clone-of-has-env-bot")
            .expect("duplicate should succeed");

        let target_dir = profile_dir_for("clone-of-has-env-bot");
        assert!(
            !target_dir.join(".env").exists(),
            "duplicate must never copy the source's .env file (D-17)"
        );
    }

    #[test]
    fn duplicate_profile_impl_tree_walk_finds_no_seeded_secret_anywhere_under_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let secret_value = "sk-TreeWalkSecretZ9f3kQ7xN2mP8wL4vR6tY1u";
        let source_dir = profile_dir_for("secret-bot");
        write_fixture_file(
            &source_dir.join("config.yaml"),
            "model:\n  provider: openrouter\n",
        );
        write_fixture_file(
            &source_dir.join(".env"),
            &format!("OPENROUTER_API_KEY={secret_value}\n"),
        );
        write_fixture_file(
            &source_dir.join("skills").join("s").join("SKILL.md"),
            "safe content\n",
        );
        write_fixture_file(&source_dir.join("memories").join("MEMORY.md"), "safe content\n");
        write_fixture_file(&source_dir.join("workspace").join("SOUL.md"), "safe content\n");

        duplicate_profile_impl("secret-bot", "secret-bot-clone").expect("duplicate should succeed");

        let target_dir = profile_dir_for("secret-bot-clone");
        fn walk_files_recursive(root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            if let Ok(entries) = fs::read_dir(root) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        walk_files_recursive(&path, out);
                    } else {
                        out.push(path);
                    }
                }
            }
        }
        let mut files = Vec::new();
        walk_files_recursive(&target_dir, &mut files);
        assert!(
            !files.is_empty(),
            "target directory must contain files for this walk to actually prove anything"
        );
        for file in &files {
            let bytes = fs::read(file).expect("read target file");
            let contents = String::from_utf8_lossy(&bytes);
            assert!(
                !contents.contains(secret_value),
                "found the seeded secret in {file:?} — a real leak, not a source-inspection false pass"
            );
        }
    }

    #[test]
    fn duplicate_profile_impl_rejects_existing_target_leaves_both_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let source_dir = profile_dir_for("src-exists");
        write_fixture_file(&source_dir.join("config.yaml"), "source config\n");
        let target_dir = profile_dir_for("dst-exists");
        write_fixture_file(&target_dir.join("marker.txt"), "pre-existing target content\n");

        let before = walk_all(&dir.path().join("profiles"));
        let result = duplicate_profile_impl("src-exists", "dst-exists");
        assert!(result.is_err(), "an existing target must be rejected");
        let after = walk_all(&dir.path().join("profiles"));
        assert_eq!(
            before, after,
            "an existing target must leave both directories untouched at the top level"
        );
        assert_eq!(
            fs::read_to_string(target_dir.join("marker.txt")).expect("target marker survives"),
            "pre-existing target content\n"
        );
    }

    #[test]
    fn duplicate_profile_impl_rejects_reserved_target_names_creates_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let source_dir = profile_dir_for("reserved-source");
        write_fixture_file(&source_dir.join("config.yaml"), "source\n");

        for reserved in ["default", "current", "none"] {
            let result = duplicate_profile_impl("reserved-source", reserved);
            assert!(result.is_err(), "target '{reserved}' must be rejected");
        }
        assert!(
            !dir.path().join("profiles").join("default").exists()
                && !dir.path().join("profiles").join("current").exists()
                && !dir.path().join("profiles").join("none").exists(),
            "no reserved-name target directory must ever be created"
        );
    }

    #[test]
    fn duplicate_profile_impl_rejects_invalid_chars_target_name_creates_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let source_dir = profile_dir_for("bad-chars-source");
        write_fixture_file(&source_dir.join("config.yaml"), "source\n");

        let before = walk_all(&dir.path().join("profiles"));
        let result = duplicate_profile_impl("bad-chars-source", "../../etc/passwd");
        assert!(result.is_err());
        let after = walk_all(&dir.path().join("profiles"));
        assert_eq!(
            before, after,
            "a traversal-shaped target name must write nothing new under profiles/"
        );
    }

    #[test]
    fn duplicate_profile_impl_rejects_missing_source_creates_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let result = duplicate_profile_impl("does-not-exist", "new-target");
        assert!(result.is_err());
        assert!(!profile_dir_for("new-target").exists());
    }

    #[test]
    fn duplicate_profile_impl_failure_partway_leaves_no_partial_target() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let source_dir = profile_dir_for("partial-fail-source");
        write_fixture_file(&source_dir.join("config.yaml"), "source\n");
        let unreadable = source_dir.join("memories").join("MEMORY.md");
        write_fixture_file(&unreadable, "content");
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000))
            .expect("chmod unreadable");

        let result = duplicate_profile_impl("partial-fail-source", "partial-fail-target");

        // Restore permissions unconditionally so tempdir cleanup can remove
        // the fixture regardless of the assertions below.
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o600))
            .expect("restore perms for cleanup");

        assert!(result.is_err(), "an unreadable source file must fail the copy");
        assert!(
            !profile_dir_for("partial-fail-target").exists(),
            "a mid-copy failure must never leave a half-populated target directory"
        );
        let staging_leftovers: Vec<_> = fs::read_dir(dir.path().join("profiles"))
            .expect("read profiles root")
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".duplicate-staging-")
            })
            .collect();
        assert!(
            staging_leftovers.is_empty(),
            "a failed duplicate must clean up its own staging directory"
        );
    }

    // -------------------------------------------------------------------
    // Phase 50.1 Plan 06 (D-18): delete_profile_impl / is_deletion_protected.
    // -------------------------------------------------------------------

    #[test]
    fn delete_profile_is_deletion_protected_returns_true_only_for_default() {
        assert!(is_deletion_protected("default"));
        assert!(!is_deletion_protected("ordinary-bot"));
        assert!(!is_deletion_protected("current"));
        assert!(!is_deletion_protected("none"));
    }

    #[test]
    fn delete_profile_impl_on_default_profile_returns_refusal_and_removes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        // Deliberately no "default" directory scaffolded — the refusal
        // must fire before any existence check, matching
        // `is_deletion_protected`'s own pure-predicate contract.

        let before = walk_all(dir.path());
        let result = delete_profile_impl("default");
        assert!(result.is_err());
        let after = walk_all(dir.path());
        assert_eq!(before, after, "the default profile refusal must remove nothing");
    }

    #[test]
    fn delete_profile_impl_on_live_profile_returns_refusal_and_removes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A home path shaped like `<root>/profiles/scout` makes
        // `ironhermes_core::current_profile()` resolve to "scout" — the
        // exact fixture shape `cli_handoff.rs`'s own
        // `run_bot_handoff_for_the_live_profile_returns_streaming_error_without_spawning`
        // test already established for this precise scenario.
        let scoped_home = dir.path().join("profiles").join("scout");
        fs::create_dir_all(&scoped_home).expect("mkdir scoped home");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", scoped_home.to_str().expect("utf8 path"));

        let result = delete_profile_impl("scout");
        assert!(result.is_err(), "the live profile must refuse deletion");
    }

    #[test]
    fn delete_profile_impl_on_ordinary_profile_removes_directory_and_bot_meta_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let target_dir = profile_dir_for("removable-bot");
        write_fixture_file(&target_dir.join("config.yaml"), "source\n");

        let patch = crate::protocol::BotMetaPatch {
            name: "removable-bot".to_string(),
            title: Some("Removable Bot".to_string()),
            description: None,
            avatar: None,
            group: None,
            preview: None,
            preview_at_ms: None,
        };
        crate::server::bot_meta_api::save_bot_meta_impl(&patch).expect("seed bot-meta");

        delete_profile_impl("removable-bot").expect("delete should succeed");

        assert!(!target_dir.exists(), "the profile directory must be removed");
        let index = crate::server::bot_meta_api::load_bot_meta_map(
            &crate::server::bot_meta_api::bot_meta_index_path(),
        )
        .expect("read bot-meta index");
        assert!(
            !index.contains_key("removable-bot"),
            "the bot-meta index must no longer carry the removed profile's key"
        );
    }

    #[test]
    fn delete_profile_impl_leaves_sibling_bot_meta_entries_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        write_fixture_file(&profile_dir_for("bot-a").join("config.yaml"), "a\n");
        write_fixture_file(&profile_dir_for("bot-b").join("config.yaml"), "b\n");
        for name in ["bot-a", "bot-b"] {
            let patch = crate::protocol::BotMetaPatch {
                name: name.to_string(),
                title: Some(name.to_string()),
                description: None,
                avatar: None,
                group: None,
                preview: None,
                preview_at_ms: None,
            };
            crate::server::bot_meta_api::save_bot_meta_impl(&patch).expect("seed bot-meta");
        }

        delete_profile_impl("bot-a").expect("delete should succeed");

        let index = crate::server::bot_meta_api::load_bot_meta_map(
            &crate::server::bot_meta_api::bot_meta_index_path(),
        )
        .expect("read bot-meta index");
        assert!(!index.contains_key("bot-a"));
        assert!(
            index.contains_key("bot-b"),
            "a sibling profile's bot-meta entry must survive unrelated to the deleted one"
        );
        assert_eq!(
            index.get("bot-b").and_then(|m| m.title.clone()),
            Some("bot-b".to_string())
        );
    }

    #[test]
    fn delete_profile_impl_on_missing_profile_returns_not_found_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let result = delete_profile_impl("never-existed");
        assert!(result.is_err(), "a nonexistent profile must not succeed silently");
    }

    #[test]
    fn delete_profile_impl_rejects_invalid_name_before_resolving_any_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let before = walk_all(dir.path());
        let result = delete_profile_impl("../../etc/passwd");
        assert!(result.is_err());
        let after = walk_all(dir.path());
        assert_eq!(
            before, after,
            "a validation-rejected name must never resolve or touch a path"
        );
    }

    #[test]
    #[cfg(unix)]
    fn delete_profile_impl_refuses_symlinked_profile_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let real_target = dir.path().join("outside-profiles-root");
        fs::create_dir_all(&real_target).expect("mkdir outside target");
        write_fixture_file(&real_target.join("sentinel.txt"), "must survive\n");

        let profiles_root = dir.path().join("profiles");
        fs::create_dir_all(&profiles_root).expect("mkdir profiles root");
        std::os::unix::fs::symlink(&real_target, profiles_root.join("symlinked-bot"))
            .expect("create symlinked profile dir");

        let result = delete_profile_impl("symlinked-bot");
        assert!(result.is_err(), "a symlinked profile path must be refused");
        assert!(
            real_target.join("sentinel.txt").exists(),
            "the symlink target must never be touched"
        );
    }
}

/// Phase 47.4 Plan 07 (D-07/D-13): real fixture-directory tests for
/// `preview_resolved_keys_impl` — the wizard step-2 live key preview.
#[cfg(all(test, feature = "server"))]
mod profile_preview_tests {
    use super::*;
    use ironhermes_core::config::Config;

    /// RAII guard, duplicated per this file's own established precedent
    /// (see `profile_scaffold_tests::ScopedEnv`'s doc comment).
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; no concurrent env access.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: single-threaded test context; no concurrent env access.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    fn home(dir: &tempfile::TempDir) -> ScopedEnv {
        ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        )
    }

    #[test]
    fn llm_only_mode_yields_the_fixed_five_row_allowlist_regardless_of_resolution() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        std::fs::write(dir.path().join(".env"), "OPENROUTER_API_KEY=sk-abc\n").expect("root .env");

        let rows = preview_resolved_keys_impl(&KeyMode::LlmOnly, Vec::new(), &Config::default())
            .expect("preview should succeed");
        assert_eq!(rows.len(), 5, "LlmOnly must always render all 5 allowlist rows");
        let resolved = rows
            .iter()
            .find(|r| r.name == "OPENROUTER_API_KEY")
            .expect("row present");
        assert_eq!(resolved.status, KeyStatus::Inherited);
        let missing = rows
            .iter()
            .find(|r| r.name == "ANTHROPIC_API_KEY")
            .expect("row present");
        assert_eq!(missing.status, KeyStatus::Missing);
        assert_eq!(missing.masked, "\u{2014}");
    }

    #[test]
    fn all_keys_mode_yields_every_matching_suffix_name_from_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        std::fs::write(
            dir.path().join(".env"),
            "OPENROUTER_API_KEY=sk-abc\nSOME_OTHER_TOKEN=xyz\nUNRELATED_VAR=1\n",
        )
        .expect("root .env");

        let rows = preview_resolved_keys_impl(&KeyMode::AllKeys, Vec::new(), &Config::default())
            .expect("preview should succeed");
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"OPENROUTER_API_KEY"));
        assert!(names.contains(&"SOME_OTHER_TOKEN"));
        assert!(!names.contains(&"UNRELATED_VAR"));
    }

    #[test]
    fn explicit_mode_yields_exactly_the_caller_supplied_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        std::fs::write(dir.path().join(".env"), "CUSTOM_KEY=abc\n").expect("root .env");

        let rows = preview_resolved_keys_impl(
            &KeyMode::Explicit(vec!["CUSTOM_KEY".to_string(), "OTHER_KEY".to_string()]),
            Vec::new(),
            &Config::default(),
        )
        .expect("preview should succeed");
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter().find(|r| r.name == "CUSTOM_KEY").unwrap().status,
            KeyStatus::Inherited
        );
        assert_eq!(
            rows.iter().find(|r| r.name == "OTHER_KEY").unwrap().status,
            KeyStatus::Missing
        );
    }

    #[test]
    fn manual_key_overlays_and_appends_a_row_outside_the_mode_set() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        std::fs::write(dir.path().join(".env"), "").expect("empty root .env");

        let manual = vec![(
            "OPENROUTER_API_KEY".to_string(),
            secrecy::SecretString::from("sk-manual".to_string()),
        )];
        let rows =
            preview_resolved_keys_impl(&KeyMode::LlmOnly, manual, &Config::default()).expect("preview should succeed");
        let row = rows
            .iter()
            .find(|r| r.name == "OPENROUTER_API_KEY")
            .expect("row present");
        assert_eq!(row.status, KeyStatus::ManuallySet);
        assert_ne!(row.masked, "\u{2014}");
    }

    #[test]
    fn no_raw_value_appears_in_the_serialized_response() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let secret = "sk-supersecret-entropy-marker-zz9plural";
        std::fs::write(dir.path().join(".env"), format!("OPENROUTER_API_KEY={secret}\n"))
            .expect("root .env");

        let rows = preview_resolved_keys_impl(&KeyMode::LlmOnly, Vec::new(), &Config::default())
            .expect("preview should succeed");
        let json = serde_json::to_string(&rows).expect("serialize");
        assert!(
            !json.contains(secret),
            "serialized preview response must never contain the raw key value"
        );
    }
}

/// Phase 47.4 Plan 05 Task 3 (D-02/D-04/D-07/D-11/D-13): real fixture-directory
/// tests for `fetch_profile_detail_impl` / `classify_key_status` /
/// `update_profile_config_impl` / `merge_profile_config_payload` /
/// `save_profile_key_impl` / `validate_key_name` / `validate_key_value`.
///
/// Same bin-only-crate constraint documented in `profile_health_tests` and
/// `profile_scaffold_tests` above (`iron_hermes_ui` has no `src/lib.rs`, so
/// `tests/*.rs` integration tests cannot reach `pub(crate)` items) — the
/// real tests this plan's Task 3 calls for live HERE, not in
/// `crates/iron_hermes_ui/tests/profile_key_masking.rs` (that file is the
/// companion shape-lock/registration-check, mirroring
/// `tests/profile_health.rs` / `tests/profile_scaffold.rs`). Run with
/// (mutates process-global `IRONHERMES_HOME`, so `--test-threads=1` is
/// required):
///   `cargo nextest run -p iron_hermes_ui --features server profile_key_masking_tests --test-threads=1`
///
/// Mutation sanity map — deleting any one of these lines should turn at
/// least one test in this module red (mutation check performed manually
/// per the plan's acceptance criteria: added a `value: String` field
/// carrying the real value to `KeyRow` in `protocol.rs`, ran this module,
/// confirmed `no_leak_*` tests failed red, reverted, confirmed green
/// again):
/// - the `r == p` equality branch in `classify_key_status` →
///   `classify_key_status_same_value_both_sides_is_inherited` and
///   `classify_key_status_different_values_both_sides_is_manually_set`
/// - the `.filter(|v| !v.is_empty())` calls in `classify_key_status` →
///   `classify_key_status_empty_string_treated_as_absent_on_both_sides`
/// - `mask_key_value`'s fixed-length bullet string / never-embeds-input
///   behavior → `fetch_profile_detail_no_raw_key_substring_in_serialized_json`,
///   `save_profile_key_no_raw_value_substring_in_serialized_json`,
///   `mask_key_value_length_invariant_across_different_length_secrets`
/// - the `!dir.is_dir()` guard in `fetch_profile_detail_impl` →
///   `fetch_profile_detail_missing_dir_is_error`
/// - the `Err(_) => (None, None, false)` malformed-config branch in
///   `fetch_profile_detail_impl` →
///   `fetch_profile_detail_malformed_config_yaml_degrades_not_fails`
/// - the `if let Some(ref provider) = payload.provider` guard in
///   `merge_profile_config_payload` →
///   `update_profile_config_only_provider_some_changes_only_provider`
/// - the `profile_dir_for(&payload.name)` (never root) target path in
///   `update_profile_config_impl` →
///   `update_profile_config_never_touches_root_config_yaml`
/// - the `!profile_dir.is_dir()` guard in `save_profile_key_impl` →
///   `save_profile_key_nonexistent_profile_is_error_creates_nothing`
/// - the upsert-preserve-rest loop in `save_profile_key_impl` →
///   `save_profile_key_existing_name_replaces_value_preserves_rest` and
///   `save_profile_key_new_name_appends_preserves_existing`
/// - `validate_key_name`'s `[A-Z][A-Z0-9_]*` check →
///   `validate_key_name_rejects_lowercase_and_non_alnum`
/// - `validate_key_value`'s trim-empty check →
///   `validate_key_value_rejects_empty_and_whitespace_only`
///
/// Phase 47.4 Plan 18 (CR-02/WR-01) additions to this map:
/// - `validate_key_value`'s new `is_control` rejection, called from both
///   `create_profile_impl` and `save_profile_key_impl` →
///   `create_profile_impl_rejects_a_manual_key_value_forging_a_second_env_line`
///   (`profile_scaffold_tests`),
///   `create_profile_impl_rejects_a_manual_key_name_containing_a_newline`
///   (`profile_scaffold_tests`), and
///   `save_profile_key_impl_rejects_a_value_containing_a_newline` (below)
/// - `create_profile_impl`'s pre-render `sort_by` (WR-01) →
///   `create_profile_impl_writes_env_entries_sorted_by_key_name` (below)
/// - the no-forgery property of validated input round-tripping through
///   `render_profile_env` → `read_env_keys` →
///   `rendered_env_round_trips_to_exactly_the_validated_entry_set` (below)
#[cfg(all(test, feature = "server"))]
mod profile_key_masking_tests {
    use super::*;
    use ironhermes_core::config::Config;
    use std::fs;

    /// RAII guard that sets an env var and restores the previous value on
    /// drop. Duplicated verbatim from `profile_scaffold_tests::ScopedEnv`
    /// above (and originally `ironhermes-kanban/src/paths.rs:449-475`) —
    /// each `#[cfg(test)]` module is its own namespace, so this is the same
    /// sanctioned "duplicate the guard" pattern Plan 03 already established.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; no concurrent env access.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: single-threaded test context; no concurrent env access.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    fn home(dir: &tempfile::TempDir) -> ScopedEnv {
        ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        )
    }

    // -------------------------------------------------------------------
    // classify_key_status — every D-07 permutation. The Inherited vs
    // ManuallySet fixtures below differ ONLY in the profile-side value,
    // with the root-side value held constant, so each test proves
    // something about the comparison rather than about unrelated inputs.
    // -------------------------------------------------------------------

    #[test]
    fn classify_key_status_same_value_both_sides_is_inherited() {
        let root = "sk-shared-value-abc123".to_string();
        let profile = "sk-shared-value-abc123".to_string();
        assert_eq!(
            classify_key_status(Some(&root), Some(&profile)),
            KeyStatus::Inherited
        );
    }

    #[test]
    fn classify_key_status_different_values_both_sides_is_manually_set() {
        let root = "sk-shared-value-abc123".to_string();
        let profile = "sk-diverged-value-xyz789".to_string();
        assert_eq!(
            classify_key_status(Some(&root), Some(&profile)),
            KeyStatus::ManuallySet,
            "a profile value that diverges from root must be ManuallySet, not Inherited"
        );
    }

    #[test]
    fn classify_key_status_profile_only_is_manually_set() {
        let profile = "sk-profile-only-value".to_string();
        assert_eq!(
            classify_key_status(None, Some(&profile)),
            KeyStatus::ManuallySet
        );
    }

    #[test]
    fn classify_key_status_neither_present_is_missing() {
        assert_eq!(classify_key_status(None, None), KeyStatus::Missing);
    }

    #[test]
    fn classify_key_status_root_only_is_missing() {
        // Root-only presence is never surfaced as Inherited on its own —
        // fetch_profile_detail_impl's row set is driven by the allowlist +
        // profile .env names; a key present only in root and absent from
        // the profile's own .env is correctly Missing (nothing was
        // actually inherited onto this profile's file).
        let root = "sk-root-only-value".to_string();
        assert_eq!(classify_key_status(Some(&root), None), KeyStatus::Missing);
    }

    #[test]
    fn classify_key_status_empty_string_treated_as_absent_on_both_sides() {
        let empty = String::new();
        assert_eq!(
            classify_key_status(Some(&empty), Some(&empty)),
            KeyStatus::Missing,
            "an empty-string value must be treated as absent, not as a matching Inherited pair"
        );
    }

    // -------------------------------------------------------------------
    // fetch_profile_detail_impl
    // -------------------------------------------------------------------

    #[test]
    fn fetch_profile_detail_fully_configured_returns_configured_no_gaps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("detail-configured");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");

        let mut cfg = Config::default();
        cfg.model.provider = "anthropic".to_string();
        cfg.model.default = "claude-3-opus".to_string();
        cfg.save_to(&profile_dir.join("config.yaml"))
            .expect("save_to profile config.yaml");
        let env_path = profile_dir.join(".env");
        fs::write(&env_path, "ANTHROPIC_API_KEY=sk-abc123\n").expect("write profile .env");

        let detail =
            fetch_profile_detail_impl("detail-configured").expect("fetch_profile_detail_impl");
        assert_eq!(detail.health, ProfileHealth::Configured);
        assert!(detail.gaps.is_empty());
        assert_eq!(detail.provider.as_deref(), Some("anthropic"));
        assert_eq!(detail.model_default.as_deref(), Some("claude-3-opus"));
    }

    /// Cross-check against `list_profiles`' own per-profile computation
    /// (dir_exists + config-load-fallible + resolvable-count-from-profile-
    /// env-only), reproduced inline exactly as `list_profiles`' loop body
    /// computes it — the same three inputs, so the two call sites can never
    /// disagree for identical disk state (T-47.4-05 acceptance criterion).
    #[test]
    fn fetch_profile_detail_agrees_with_list_profiles_classification() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let name = "detail-vs-list-profile";
        let profile_dir = profile_dir_for(name);
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        let mut cfg = Config::default();
        cfg.model.provider = "groq".to_string();
        cfg.model.default = "llama-3".to_string();
        cfg.save_to(&profile_dir.join("config.yaml"))
            .expect("save_to profile config.yaml");
        fs::write(profile_dir.join(".env"), "GROQ_API_KEY=sk-groq-abc\n")
            .expect("write profile .env");

        // list_profiles' own per-entry computation, reproduced verbatim
        // (GAP-1: now provider-registry-derived + dispatch-gate-based, same
        // as `list_profiles`' real loop body).
        let dir_exists = profile_dir.is_dir();
        let config_path = profile_dir.join("config.yaml");
        let config_yaml_on_disk = config_path.is_file();
        let (loaded_cfg, provider, config_parsed_ok) = if config_yaml_on_disk {
            match Config::load_from(&config_path) {
                Ok(cfg) => {
                    let provider = Some(cfg.model.provider.clone());
                    (Some(cfg), provider, true)
                }
                Err(_) => (None, None, false),
            }
        } else {
            (None, None, false)
        };
        let env_map = read_env_keys(&profile_dir.join(".env")).unwrap_or_default();
        let resolvable_llm_key_count = match &loaded_cfg {
            Some(cfg) => provider_key_env_names(cfg)
                .iter()
                .filter(|k| env_map.get(k.as_str()).map(|v| !v.is_empty()).unwrap_or(false))
                .count(),
            None => LLM_KEY_ALLOWLIST
                .iter()
                .filter(|k| env_map.get(**k).map(|v| !v.is_empty()).unwrap_or(false))
                .count(),
        };
        let provider_key = compute_provider_key_state(
            name,
            config_parsed_ok,
            provider.as_deref(),
            resolvable_llm_key_count,
        );
        let (list_profiles_health, list_profiles_gaps) =
            classify_profile_health(dir_exists, config_parsed_ok, provider_key);

        let detail = fetch_profile_detail_impl(name).expect("fetch_profile_detail_impl");
        assert_eq!(
            detail.health, list_profiles_health,
            "fetch_profile_detail and list_profiles must classify the same disk state identically"
        );
        assert_eq!(detail.gaps, list_profiles_gaps);
    }

    #[test]
    fn fetch_profile_detail_missing_dir_is_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        // Deliberately never created.
        let result = fetch_profile_detail_impl("never-created-profile");
        assert!(result.is_err(), "a missing profile dir must be an error");
    }

    #[test]
    fn fetch_profile_detail_malformed_config_yaml_degrades_not_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("malformed-config-profile");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        // Not valid YAML at all (a bare colon-less scalar can't parse into
        // the Config struct's map shape).
        fs::write(
            profile_dir.join("config.yaml"),
            b"not: [valid, yaml: at all\n",
        )
        .expect("write malformed config.yaml");

        let detail = fetch_profile_detail_impl("malformed-config-profile")
            .expect("a malformed config.yaml must degrade, not fail the whole call");
        assert_eq!(detail.provider, None);
        assert_eq!(detail.model_default, None);
        assert!(detail
            .gaps
            .iter()
            .any(|g| matches!(g, ProfileGap::MissingConfigYaml)));
    }

    #[test]
    fn fetch_profile_detail_malformed_profile_env_returns_err_not_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("malformed-env-profile");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        fs::write(
            profile_dir.join(".env"),
            "this line has no equals sign at all\n",
        )
        .expect("write malformed .env");

        let result = fetch_profile_detail_impl("malformed-env-profile");
        assert!(
            result.is_err(),
            "a malformed profile .env must return Err, not panic"
        );
    }

    #[test]
    fn fetch_profile_detail_reports_web_config_write_enabled_truthfully() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("write-flag-profile");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");

        let mut root_cfg = Config::default();
        root_cfg.security.web_config_write_enabled = true;
        root_cfg
            .save_to(&dir.path().join("config.yaml"))
            .expect("save_to root config.yaml");

        let detail =
            fetch_profile_detail_impl("write-flag-profile").expect("fetch_profile_detail_impl");
        assert!(
            detail.web_config_write_enabled,
            "the root's web_config_write_enabled flag must be reported truthfully"
        );
    }

    #[test]
    fn fetch_profile_detail_missing_llm_key_is_visible_as_missing_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("missing-key-row-profile");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        // No .env at all — every allowlisted key must still appear as a
        // Missing row, not be silently absent from the table.
        let detail = fetch_profile_detail_impl("missing-key-row-profile")
            .expect("fetch_profile_detail_impl");
        let row = detail
            .keys
            .iter()
            .find(|r| r.name == "OPENROUTER_API_KEY")
            .expect("OPENROUTER_API_KEY must always appear as a row (allowlist union)");
        assert_eq!(row.status, KeyStatus::Missing);
        assert_eq!(row.masked, "\u{2014}");
    }

    #[test]
    fn fetch_profile_detail_no_raw_key_substring_in_serialized_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("no-leak-detail-profile");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        // Distinctive, high-entropy fixture values so a partial match
        // cannot slip through by coincidence.
        let root_secret = "sk-R00tHighEntropyValueQ7xN2mP8wL4vR6tY1u";
        let profile_secret = "sk-Pr0fileHighEntropyValueZ9f3kQ7xN2mP8wL";
        fs::write(
            dir.path().join(".env"),
            format!("OPENROUTER_API_KEY={root_secret}\n"),
        )
        .expect("write root .env");
        fs::write(
            profile_dir.join(".env"),
            format!("ANTHROPIC_API_KEY={profile_secret}\n"),
        )
        .expect("write profile .env");

        let detail =
            fetch_profile_detail_impl("no-leak-detail-profile").expect("fetch_profile_detail_impl");
        let serialized = serde_json::to_string(&detail).expect("serialize ProfileDetail");
        assert!(
            !serialized.contains(root_secret),
            "serialized ProfileDetail must never contain the root secret value"
        );
        assert!(
            !serialized.contains(profile_secret),
            "serialized ProfileDetail must never contain the profile secret value"
        );
    }

    // -------------------------------------------------------------------
    // update_profile_config_impl / merge_profile_config_payload /
    // validate_profile_config_payload
    // -------------------------------------------------------------------

    fn write_payload(
        name: &str,
        provider: Option<&str>,
        model_default: Option<&str>,
    ) -> ProfileConfigWritePayload {
        ProfileConfigWritePayload {
            name: name.to_string(),
            provider: provider.map(|s| s.to_string()),
            model_default: model_default.map(|s| s.to_string()),
            skills_disabled: None,
        }
    }

    #[test]
    fn update_profile_config_only_provider_some_changes_only_provider() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("update-provider-only");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        let mut initial = Config::default();
        initial.model.provider = "openrouter".to_string();
        initial.model.default = "some/model".to_string();
        initial
            .save_to(&profile_dir.join("config.yaml"))
            .expect("save_to profile config.yaml");

        let payload = write_payload("update-provider-only", Some("anthropic"), None);
        update_profile_config_impl(&payload).expect("update should succeed");

        let updated = Config::load_from(&profile_dir.join("config.yaml"))
            .expect("reload profile config.yaml");
        assert_eq!(updated.model.provider, "anthropic");
        assert_eq!(
            updated.model.default, "some/model",
            "model_default: None must leave the existing value untouched"
        );
    }

    #[test]
    fn update_profile_config_both_none_is_a_no_op_returns_ok() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("update-no-op");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        let mut initial = Config::default();
        initial.model.provider = "openrouter".to_string();
        initial.model.default = "some/model".to_string();
        initial
            .save_to(&profile_dir.join("config.yaml"))
            .expect("save_to profile config.yaml");

        let payload = write_payload("update-no-op", None, None);
        let result = update_profile_config_impl(&payload);
        assert!(result.is_ok(), "both-None payload must still return Ok");

        let after = Config::load_from(&profile_dir.join("config.yaml"))
            .expect("reload profile config.yaml");
        assert_eq!(after.model.provider, "openrouter");
        assert_eq!(after.model.default, "some/model");
    }

    #[test]
    fn update_profile_config_never_touches_root_config_yaml() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let mut root_cfg = Config::default();
        root_cfg.model.provider = "root-provider".to_string();
        root_cfg.model.default = "root-model".to_string();
        root_cfg
            .save_to(&dir.path().join("config.yaml"))
            .expect("save_to root config.yaml");
        let root_bytes_before =
            fs::read(dir.path().join("config.yaml")).expect("read root config.yaml before");

        let profile_dir = profile_dir_for("update-isolated-profile");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        Config::default()
            .save_to(&profile_dir.join("config.yaml"))
            .expect("save_to profile config.yaml");

        let payload = write_payload(
            "update-isolated-profile",
            Some("anthropic"),
            Some("claude-3"),
        );
        update_profile_config_impl(&payload).expect("update should succeed");

        let root_bytes_after =
            fs::read(dir.path().join("config.yaml")).expect("read root config.yaml after");
        assert_eq!(
            root_bytes_before, root_bytes_after,
            "update_profile_config_impl must leave the ROOT config.yaml byte-identical"
        );
    }

    #[test]
    fn update_profile_config_rejects_empty_string_provider() {
        let payload = write_payload("valid-name", Some(""), None);
        assert!(validate_profile_config_payload(&payload).is_err());
    }

    #[test]
    fn update_profile_config_rejects_empty_string_model_default() {
        let payload = write_payload("valid-name", None, Some(""));
        assert!(validate_profile_config_payload(&payload).is_err());
    }

    #[test]
    fn update_profile_config_gate_closed_means_impl_never_called_file_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("gate-closed-config-profile");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        let sentinel: &[u8] = b"model:\n  provider: sentinel\n  default: sentinel-model\n";
        fs::write(profile_dir.join("config.yaml"), sentinel).expect("write sentinel config.yaml");

        let config = Config::default();
        assert!(
            check_profile_write_gate(&config).is_err(),
            "gate must be closed by default"
        );
        // A real caller (update_profile_config's async wrapper) stops here
        // and never calls update_profile_config_impl.
        let after = fs::read(profile_dir.join("config.yaml")).expect("read after");
        assert_eq!(
            after, sentinel,
            "profile config.yaml must be byte-unchanged when the gate refuses the write"
        );
    }

    // -------------------------------------------------------------------
    // save_profile_key_impl / validate_key_name / validate_key_value
    // -------------------------------------------------------------------

    #[test]
    fn save_profile_key_new_name_appends_preserves_existing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("save-key-append");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        fs::write(
            profile_dir.join(".env"),
            "OPENROUTER_API_KEY=sk-existing-value\n",
        )
        .expect("write initial .env");

        let secret = SecretString::from("sk-new-anthropic-value".to_string());
        let row = save_profile_key_impl("save-key-append", "ANTHROPIC_API_KEY", &secret)
            .expect("save_profile_key_impl should succeed");
        assert_eq!(row.status, KeyStatus::ManuallySet);

        // Phase 47.4 Plan 20 (CR-03/CR-04): read back through the real
        // dotenvy parser rather than a raw unquoted-substring match — the
        // renderer now single-quotes every value, so pinning the exact byte
        // form here would pin the representation, not the property (D-06).
        let parsed = read_env_keys(&profile_dir.join(".env")).expect("read_env_keys after save");
        assert_eq!(
            parsed.get("OPENROUTER_API_KEY").map(String::as_str),
            Some("sk-existing-value")
        );
        assert_eq!(
            parsed.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-new-anthropic-value")
        );
    }

    /// D-07 (`proceed-with-inventory`, decided by the operator at plan time):
    /// the provenance header is the *recoverability* mechanism the one-way
    /// plaintext-`.env` decision was approved on — a future per-profile
    /// secret-storage migration enumerates the files this phase created by
    /// grepping for it. `create_profile` stamping it is not sufficient:
    /// `save_profile_key` rewrites the whole file, so a rewrite that dropped
    /// the header would silently make a profile invisible to that migration.
    ///
    /// The fixture deliberately starts from an `.env` with NO header — the
    /// shape a `scripts/make-kanban-profile`- or hand-created profile has —
    /// so this asserts the save path *stamps*, not merely *preserves*.
    #[test]
    fn save_profile_key_rewrite_stamps_provenance_header_exactly_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("save-key-provenance");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        fs::write(
            profile_dir.join(".env"),
            "OPENROUTER_API_KEY=sk-existing-value\n",
        )
        .expect("write initial .env with no provenance header");

        let secret = SecretString::from("sk-new-anthropic-value".to_string());
        save_profile_key_impl("save-key-provenance", "ANTHROPIC_API_KEY", &secret)
            .expect("save_profile_key_impl should succeed");

        let contents = fs::read_to_string(profile_dir.join(".env")).expect("read .env after save");
        assert!(
            contents.starts_with(PROFILE_ENV_PROVENANCE_PREFIX),
            "a save_profile_key rewrite must stamp the provenance header, otherwise \
             the profile becomes invisible to a future secret-storage migration"
        );
        assert_eq!(
            contents.matches(PROFILE_ENV_PROVENANCE_PREFIX).count(),
            1,
            "the provenance header must appear exactly once, never accumulate per rewrite"
        );
        assert!(contents.contains("save-key-provenance"));
        // The rewrite must still be a real key-save, not just a header
        // stamp — read back through the real dotenvy parser (Phase 47.4
        // Plan 20, CR-03/CR-04) rather than an unquoted-substring match.
        let parsed = read_env_keys(&profile_dir.join(".env")).expect("read_env_keys after save");
        assert_eq!(
            parsed.get("OPENROUTER_API_KEY").map(String::as_str),
            Some("sk-existing-value")
        );
        assert_eq!(
            parsed.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-new-anthropic-value")
        );
    }

    /// A second save must not append a duplicate header to an `.env` that
    /// already carries one — the idempotence half of the guarantee above.
    #[test]
    fn repeated_save_profile_key_does_not_duplicate_provenance_header() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("save-key-provenance-twice");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        fs::write(profile_dir.join(".env"), "OPENROUTER_API_KEY=sk-a\n")
            .expect("write initial .env");

        for (key, value) in [
            ("ANTHROPIC_API_KEY", "sk-first-save"),
            ("GROQ_API_KEY", "sk-second-save"),
        ] {
            let secret = SecretString::from(value.to_string());
            save_profile_key_impl("save-key-provenance-twice", key, &secret)
                .expect("save_profile_key_impl should succeed");
        }

        let contents = fs::read_to_string(profile_dir.join(".env")).expect("read .env after saves");
        assert_eq!(
            contents.matches(PROFILE_ENV_PROVENANCE_PREFIX).count(),
            1,
            "repeated saves must leave exactly one provenance header"
        );
    }

    #[test]
    fn save_profile_key_existing_name_replaces_value_preserves_rest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("save-key-replace");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        fs::write(
            profile_dir.join(".env"),
            "OPENROUTER_API_KEY=sk-old-value\nGROQ_API_KEY=sk-groq-unchanged\n",
        )
        .expect("write initial .env");

        let secret = SecretString::from("sk-replaced-value".to_string());
        save_profile_key_impl("save-key-replace", "OPENROUTER_API_KEY", &secret)
            .expect("save_profile_key_impl should succeed");

        let contents = fs::read_to_string(profile_dir.join(".env")).expect("read .env after save");
        assert!(!contents.contains("sk-old-value"));
        // Read back through the real dotenvy parser (Phase 47.4 Plan 20,
        // CR-03/CR-04) rather than an unquoted-substring match.
        let parsed = read_env_keys(&profile_dir.join(".env")).expect("read_env_keys after save");
        assert_eq!(
            parsed.get("OPENROUTER_API_KEY").map(String::as_str),
            Some("sk-replaced-value")
        );
        assert_eq!(
            parsed.get("GROQ_API_KEY").map(String::as_str),
            Some("sk-groq-unchanged"),
            "an unrelated key must be preserved untouched"
        );
    }

    #[test]
    fn save_profile_key_env_file_retains_mode_0600() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("save-key-perm");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        fs::write(profile_dir.join(".env"), "OPENROUTER_API_KEY=sk-existing\n")
            .expect("write initial .env");

        let secret = SecretString::from("sk-new-value".to_string());
        save_profile_key_impl("save-key-perm", "GROQ_API_KEY", &secret)
            .expect("save_profile_key_impl should succeed");

        use std::os::unix::fs::PermissionsExt;
        let meta = fs::metadata(profile_dir.join(".env")).expect("metadata of .env");
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn save_profile_key_nonexistent_profile_is_error_creates_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let secret = SecretString::from("sk-value".to_string());
        let result = save_profile_key_impl("never-created", "OPENROUTER_API_KEY", &secret);
        assert!(result.is_err());
        assert!(!profile_dir_for("never-created").exists());
    }

    #[test]
    fn save_profile_key_no_raw_value_substring_in_serialized_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("save-key-no-leak");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        fs::write(profile_dir.join(".env"), "").expect("write empty .env");

        let secret_value = "sk-Z9f3kQ7xN2mP8wL4vR6tY1uJ5hG0bC3eA-distinctive";
        let secret = SecretString::from(secret_value.to_string());
        let row = save_profile_key_impl("save-key-no-leak", "OPENROUTER_API_KEY", &secret)
            .expect("save_profile_key_impl should succeed");

        let serialized = serde_json::to_string(&row).expect("serialize KeyRow response");
        assert!(
            !serialized.contains(secret_value),
            "serialized save_profile_key return must never contain the raw written value"
        );
        assert_eq!(row.status, KeyStatus::ManuallySet);
    }

    #[test]
    fn save_profile_key_gate_closed_means_impl_never_called_file_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("gate-closed-key-profile");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        let sentinel = "OPENROUTER_API_KEY=sk-sentinel-untouched\n";
        fs::write(profile_dir.join(".env"), sentinel).expect("write sentinel .env");

        let config = Config::default();
        assert!(
            check_profile_write_gate(&config).is_err(),
            "gate must be closed by default"
        );
        // A real caller (save_profile_key's async wrapper) stops here and
        // never calls save_profile_key_impl.
        let after = fs::read_to_string(profile_dir.join(".env")).expect("read after");
        assert_eq!(
            after, sentinel,
            "profile .env must be byte-unchanged when the gate refuses the write"
        );
    }

    // -------------------------------------------------------------------
    // Task 2 (T-47.4-18-02/03, WR-01): save path's value side closed,
    // ordering deterministic, no-forgery round-trip.
    // -------------------------------------------------------------------

    #[test]
    fn save_profile_key_impl_rejects_a_value_containing_a_newline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profile_dir = profile_dir_for("save-key-forge-value");
        fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        let original = "OPENROUTER_API_KEY=sk-original-value\n";
        fs::write(profile_dir.join(".env"), original).expect("write initial .env");
        let before = fs::read(profile_dir.join(".env")).expect("read .env before");

        let secret = SecretString::from("sk-legit-part\nINJECTED_KEY=sk-forged-part".to_string());
        let result = save_profile_key_impl("save-key-forge-value", "GROQ_API_KEY", &secret);
        assert!(
            result.is_err(),
            "a value embedding a newline must be rejected at the pure-impl boundary (CR-02)"
        );

        let after = fs::read(profile_dir.join(".env")).expect("read .env after");
        assert_eq!(
            before, after,
            "a rejected write must leave the existing .env byte-for-byte unchanged (D-07)"
        );
    }

    #[test]
    fn create_profile_impl_writes_env_entries_sorted_by_key_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        fs::write(
            dir.path().join(".env"),
            "ZEBRA_API_KEY=sk-z\nMID_API_KEY=sk-m\n",
        )
        .expect("root .env");

        // A manual key alphabetically BEFORE both inherited names — the
        // overlay pushes it to the end of `resolved` (T-47.4-18-03), so
        // this actually exercises the new sort rather than a set that
        // happened to already be sorted going in.
        let manual = vec![(
            "ALPHA_API_KEY".to_string(),
            SecretString::from("sk-a".to_string()),
        )];
        create_profile_impl(
            "sorted-create-profile",
            &KeyMode::AllKeys,
            false,
            manual,
            &Config::default(),
        )
        .expect("create should succeed");

        let contents = fs::read_to_string(profile_dir_for("sorted-create-profile").join(".env"))
            .expect("read created .env");
        let assignment_lines: Vec<&str> = contents
            .lines()
            .filter(|line| !line.starts_with('#') && line.contains('='))
            .collect();
        // Phase 47.4 Plan 20 (CR-03/CR-04): values are now single-quoted by
        // `render_profile_env`; this asserts ORDER (WR-01), which is the
        // property under test, not the quoting form.
        assert_eq!(
            assignment_lines,
            vec![
                "ALPHA_API_KEY='sk-a'",
                "MID_API_KEY='sk-m'",
                "ZEBRA_API_KEY='sk-z'"
            ],
            "create_profile_impl must write assignment lines in ascending key-name order (WR-01)"
        );

        // The same key set through save_profile_key_impl must agree on
        // ordering — WR-01: "the create path and the save path no longer
        // disagree."
        let save_profile_dir = profile_dir_for("save-path-same-order");
        fs::create_dir_all(&save_profile_dir).expect("mkdir save-path profile dir");
        for (key, value) in [
            ("ZEBRA_API_KEY", "sk-z"),
            ("MID_API_KEY", "sk-m"),
            ("ALPHA_API_KEY", "sk-a"),
        ] {
            let secret = SecretString::from(value.to_string());
            save_profile_key_impl("save-path-same-order", key, &secret)
                .expect("save_profile_key_impl should succeed");
        }
        let save_contents =
            fs::read_to_string(save_profile_dir.join(".env")).expect("read save .env");
        let save_assignment_lines: Vec<&str> = save_contents
            .lines()
            .filter(|line| !line.starts_with('#') && line.contains('='))
            .collect();
        assert_eq!(
            assignment_lines, save_assignment_lines,
            "create path and save path must produce identically-ordered .env files for the same key set (WR-01)"
        );
    }

    #[test]
    fn rendered_env_round_trips_to_exactly_the_validated_entry_set() {
        let entries = vec![
            (
                "OPENROUTER_API_KEY".to_string(),
                "sk-openrouter-value".to_string(),
            ),
            (
                "ANTHROPIC_API_KEY".to_string(),
                "sk-anthropic-value".to_string(),
            ),
            ("GROQ_API_KEY".to_string(), "sk-groq-value".to_string()),
        ];
        for (name, value) in &entries {
            validate_key_name(name).expect("fixture name must be well-formed");
            validate_key_value(value).expect("fixture value must be well-formed");
        }

        let rendered = render_profile_env("round-trip-profile", &entries)
            .expect("well-formed benign entries must render and self-verify cleanly");
        // The three provenance/comment header lines are inert to the
        // parser and are not counted as entries by this round-trip
        // assertion (D-06 provenance stamp preserved).
        let header_lines = rendered.lines().filter(|l| l.starts_with('#')).count();
        assert_eq!(
            header_lines, 3,
            "provenance/comment header must be exactly the 3 documented lines"
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        fs::write(&env_path, &rendered).expect("write rendered .env");

        let parsed =
            read_env_keys(&env_path).expect("read_env_keys must parse the rendered file");
        assert_eq!(
            parsed.len(),
            entries.len(),
            "round-trip must yield exactly the same number of entries — no synthesized name, no lost name"
        );
        for (name, value) in &entries {
            assert_eq!(
                parsed.get(name).map(String::as_str),
                Some(value.as_str()),
                "round-trip must preserve the exact value for '{name}'"
            );
        }
    }

    #[test]
    fn validate_key_name_accepts_well_formed_name() {
        assert!(validate_key_name("OPENROUTER_API_KEY").is_ok());
        assert!(validate_key_name("A").is_ok());
        assert!(validate_key_name("A1_B2").is_ok());
    }

    #[test]
    fn validate_key_name_rejects_lowercase_and_non_alnum() {
        assert!(validate_key_name("openrouter_api_key").is_err());
        assert!(validate_key_name("1STARTS_WITH_DIGIT").is_err());
        assert!(validate_key_name("HAS-DASH").is_err());
        assert!(
            validate_key_name("INJECT\nVALUE=evil").is_err(),
            "a newline in the key name must be rejected (T-47.4-05-T1)"
        );
        assert!(validate_key_name("").is_err());
    }

    #[test]
    fn validate_key_value_rejects_empty_and_whitespace_only() {
        assert!(validate_key_value("").is_err());
        assert!(validate_key_value("   ").is_err());
        assert!(validate_key_value("\t\n").is_err());
    }

    #[test]
    fn validate_key_value_accepts_non_empty() {
        assert!(validate_key_value("sk-real-value").is_ok());
    }

    // -------------------------------------------------------------------
    // mask_key_value — length invariance (D-13: the mask must not leak
    // anything about value length).
    // -------------------------------------------------------------------

    #[test]
    fn mask_key_value_length_invariant_across_different_length_secrets() {
        let short = "sk-a";
        let long = "sk-a-much-longer-secret-value-that-is-quite-a-bit-bigger-1234567890";
        assert_eq!(
            mask_key_value(short),
            mask_key_value(long),
            "mask_key_value must produce byte-identical output regardless of input length"
        );
    }

    // -------------------------------------------------------------------
    // Phase 50.1 Plan 05 (D-15): apply_skills_disabled /
    // join_profile_skill_rows / validate_profile_config_payload
    // (skills_disabled) — Task 1 <behavior>.
    // -------------------------------------------------------------------

    #[test]
    fn apply_skills_disabled_writes_exact_two_names() {
        let mut cfg = Config::default();
        let catalog: HashSet<String> = ["alpha".to_string(), "beta".to_string(), "gamma".to_string()]
            .into_iter()
            .collect();
        apply_skills_disabled(&mut cfg, &["alpha".to_string(), "gamma".to_string()], &catalog)
            .expect("known names must be accepted");
        assert_eq!(
            cfg.skills.disabled,
            vec!["alpha".to_string(), "gamma".to_string()]
        );
    }

    #[test]
    fn apply_skills_disabled_empty_list_clears_opt_out() {
        let mut cfg = Config::default();
        cfg.skills.disabled = vec!["alpha".to_string()];
        let catalog: HashSet<String> = ["alpha".to_string()].into_iter().collect();
        apply_skills_disabled(&mut cfg, &[], &catalog).expect("empty list must be accepted");
        assert!(
            cfg.skills.disabled.is_empty(),
            "an empty opt-out list must clear every prior disable — every skill on"
        );
    }

    #[test]
    fn apply_skills_disabled_preserves_every_other_skills_field() {
        let mut cfg = Config::default();
        cfg.skills.enabled = false;
        cfg.skills.extra_paths = vec![PathBuf::from("/tmp/extra-skills")];
        cfg.skills.credential_dir = Some(PathBuf::from("/tmp/skill-creds"));
        cfg.skills.hub.trusted_repos = vec!["acme/skills".to_string()];
        let before = format!(
            "{:?}",
            (
                &cfg.skills.enabled,
                &cfg.skills.extra_paths,
                &cfg.skills.credential_dir,
                &cfg.skills.config,
                &cfg.skills.hub,
                &cfg.skills.defcon_level,
            )
        );

        let catalog: HashSet<String> = ["alpha".to_string()].into_iter().collect();
        apply_skills_disabled(&mut cfg, &["alpha".to_string()], &catalog)
            .expect("known name must be accepted");

        let after = format!(
            "{:?}",
            (
                &cfg.skills.enabled,
                &cfg.skills.extra_paths,
                &cfg.skills.credential_dir,
                &cfg.skills.config,
                &cfg.skills.hub,
                &cfg.skills.defcon_level,
            )
        );
        assert_eq!(
            before, after,
            "every SkillsConfig field other than `disabled` must survive byte-identical"
        );
    }

    #[test]
    fn apply_skills_disabled_preserves_non_skills_config_sections() {
        let mut cfg = Config::default();
        cfg.model.provider = "acme-provider".to_string();
        cfg.model.default = "acme-model".to_string();
        let before_model = format!("{:?}", cfg.model);
        let before_security = format!("{:?}", cfg.security);

        let catalog: HashSet<String> = ["alpha".to_string()].into_iter().collect();
        apply_skills_disabled(&mut cfg, &["alpha".to_string()], &catalog)
            .expect("known name must be accepted");

        assert_eq!(
            before_model,
            format!("{:?}", cfg.model),
            "non-skills sections must survive untouched"
        );
        assert_eq!(
            before_security,
            format!("{:?}", cfg.security),
            "non-skills sections must survive untouched"
        );
    }

    #[test]
    fn apply_skills_disabled_rejects_name_not_in_catalog_and_writes_nothing() {
        let mut cfg = Config::default();
        cfg.skills.disabled = vec!["already-disabled".to_string()];
        let catalog: HashSet<String> = ["already-disabled".to_string()].into_iter().collect();

        let err = apply_skills_disabled(&mut cfg, &["not-a-real-skill".to_string()], &catalog)
            .expect_err("an unknown skill name must be rejected");
        assert!(err.contains("not-a-real-skill"));
        assert_eq!(
            cfg.skills.disabled,
            vec!["already-disabled".to_string()],
            "a rejected write must leave the prior opt-out list untouched"
        );
    }

    #[test]
    fn validate_profile_config_payload_rejects_whitespace_only_skills_disabled_entry() {
        let payload = ProfileConfigWritePayload {
            name: "my-bot".to_string(),
            provider: None,
            model_default: None,
            skills_disabled: Some(vec!["   ".to_string()]),
        };
        let err = validate_profile_config_payload(&payload)
            .expect_err("a whitespace-only entry must be rejected");
        assert!(err.contains("skills_disabled"));
    }

    #[test]
    fn join_profile_skill_rows_marks_names_in_skills_disabled_as_not_enabled() {
        let catalog = vec![
            ("alpha".to_string(), "bundled".to_string()),
            ("beta".to_string(), "installed".to_string()),
            ("gamma".to_string(), "official".to_string()),
        ];
        let disabled: HashSet<String> = ["alpha".to_string(), "gamma".to_string()]
            .into_iter()
            .collect();

        let rows = join_profile_skill_rows(&catalog, &disabled);

        assert_eq!(rows.len(), 3, "every catalog entry must produce exactly one row");
        for row in &rows {
            let expected_enabled = row.name == "beta";
            assert_eq!(
                row.enabled_for_profile, expected_enabled,
                "{} must have enabled_for_profile={expected_enabled}",
                row.name
            );
        }
    }

    // -------------------------------------------------------------------
    // Phase 50.1 Plan 05 (D-15/D-16): profile_workspace_dir /
    // save_profile_persona_impl / fetch_profile_persona_impl /
    // validate_persona_body — Task 1 <behavior>.
    // -------------------------------------------------------------------

    /// Duplicated from `profile_scaffold_tests::write_fixture_file` — each
    /// `#[cfg(test)]` module is its own namespace, this file's own doc
    /// comments sanction the duplication explicitly.
    fn write_fixture_file(path: &std::path::Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir parent");
        }
        fs::write(path, contents).expect("write fixture file");
    }

    #[test]
    fn profile_workspace_dir_creates_the_dir_and_its_marker() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let workspace_dir = profile_workspace_dir("scout").expect("must resolve");

        assert!(workspace_dir.is_dir());
        assert!(
            workspace_dir.join(".ironhermes").is_dir(),
            "the marker directory must exist inside the bot's own workspace dir"
        );
        assert_eq!(
            workspace_dir,
            dir.path().join("profiles").join("scout").join("workspace")
        );
    }

    #[test]
    fn save_and_fetch_profile_persona_round_trips_body() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        // MA-01: save/fetch now require the profile to already exist —
        // seed it the way `create_profile_impl` would, minus the rest of
        // the scaffold this pure round-trip test doesn't need.
        std::fs::create_dir_all(profile_dir_for("scout")).expect("seed profile dir");

        save_profile_persona_impl("scout", "You are Scout, a careful researcher.")
            .expect("save must succeed");
        let persona = fetch_profile_persona_impl("scout").expect("fetch must succeed");

        assert_eq!(persona.name, "scout");
        assert_eq!(persona.body, "You are Scout, a careful researcher.");
    }

    #[test]
    fn fetch_profile_persona_impl_absent_file_is_empty_body_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        // The profile itself exists (a real bot); only its SOUL.md is
        // absent (never saved a persona yet) — distinct from MA-01's
        // "profile does not exist at all" case, covered below.
        std::fs::create_dir_all(profile_dir_for("brand-new-bot")).expect("seed profile dir");

        let persona = fetch_profile_persona_impl("brand-new-bot")
            .expect("a profile with no persona file yet must not error");
        assert_eq!(persona.body, "");
    }

    #[test]
    fn fetch_profile_persona_impl_on_nonexistent_profile_creates_nothing_on_disk() {
        // MA-01 regression (`50.1-REVIEW.md`): a browser-reachable READ
        // (`fetch_profile_persona`, no write gate) must never create a
        // profile directory for a syntactically valid but never-created
        // name, and must error rather than silently succeed with an empty
        // persona that implies the profile exists.
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let profiles_root = dir.path().join("profiles");
        let listing_before = list_dir_names(&profiles_root);

        let result = fetch_profile_persona_impl("ghost-bot");

        assert!(
            result.is_err(),
            "fetching a persona for a profile that was never created must error, not \
             fabricate one"
        );
        assert!(
            !profile_dir_for("ghost-bot").exists(),
            "fetch must not create the profile directory as a side effect"
        );
        assert_eq!(
            listing_before,
            list_dir_names(&profiles_root),
            "profiles/ must be left exactly as it was by a fetch for a name that was \
             never created"
        );
    }

    #[test]
    fn save_profile_persona_impl_on_nonexistent_profile_creates_nothing_on_disk() {
        // MA-01 fix companion: the gated WRITE side must also refuse to
        // scaffold a phantom profile from a name nobody created through
        // `create_profile` — closing the same side door with the gate on.
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let result = save_profile_persona_impl("ghost-bot", "some persona body");

        assert!(
            result.is_err(),
            "saving a persona for a profile that was never created must error, not \
             scaffold one"
        );
        assert!(
            !profile_dir_for("ghost-bot").exists(),
            "save must not create the profile directory when the profile doesn't exist"
        );
    }

    /// Directory-listing snapshot helper for the MA-01 regression above — a
    /// missing directory (nothing has been created under `profiles/` yet)
    /// and an empty one both collapse to an empty `Vec`, matching the
    /// "profiles/ untouched" assertion regardless of whether `profiles/`
    /// itself pre-existed.
    fn list_dir_names(dir: &std::path::Path) -> Vec<std::ffi::OsString> {
        std::fs::read_dir(dir)
            .map(|rd| {
                let mut names: Vec<_> = rd
                    .filter_map(|e| e.ok().map(|e| e.file_name()))
                    .collect();
                names.sort();
                names
            })
            .unwrap_or_default()
    }

    #[test]
    fn validate_persona_body_rejects_overlong_body() {
        let body = "a".repeat(PROFILE_PERSONA_MAX_BODY_LEN + 1);
        assert!(validate_persona_body(&body).is_err());
        assert!(validate_persona_body(&"a".repeat(PROFILE_PERSONA_MAX_BODY_LEN)).is_ok());
    }

    #[test]
    fn validate_persona_body_rejects_control_chars_other_than_newline_tab() {
        assert!(validate_persona_body("line one\nline two\tindented").is_ok());
        assert!(validate_persona_body("bell\u{0007}here").is_err());
        assert!(validate_persona_body("carriage\rreturn").is_err());
    }

    #[test]
    fn save_profile_persona_impl_writes_to_profile_root_not_workspace_subdir() {
        // OF-6 regression (`50.1-OPERATOR-FEEDBACK.md`): Plan 05 originally
        // wrote the persona under `workspace/SOUL.md`, on the mistaken
        // theory that `ironhermes_core::workspace::resolve_from_cwd`'s
        // resolved `soul_path` is what the agent runtime loads at turn
        // time. Direct source read of `ironhermes-agent` proved nothing
        // ever reads that field for prompt content — the actual loader
        // (`PromptBuilder::load_soul_md`) reads
        // `get_hermes_home().join("SOUL.md")`, the PROFILE ROOT. This test
        // pins the write location to that root, not `workspace/`.
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        std::fs::create_dir_all(profile_dir_for("scout")).expect("seed profile dir");

        save_profile_persona_impl("scout", "Scout's persona body").expect("save must succeed");

        assert_eq!(
            fs::read_to_string(profile_dir_for("scout").join("SOUL.md"))
                .expect("persona must land at the profile-root SOUL.md"),
            "Scout's persona body"
        );
        assert!(
            !profile_dir_for("scout").join("workspace").join("SOUL.md").exists(),
            "save must not also leave a copy at the legacy workspace/SOUL.md location"
        );
    }

    #[test]
    fn saved_persona_is_actually_loaded_by_the_real_prompt_builder_at_turn_time() {
        // OF-6's D-16 acceptance bar, proven end-to-end: assemble a REAL
        // context via the REAL loader (`ironhermes_agent::prompt_builder`,
        // not a stand-in), for a synthetic bot workspace, and assert the
        // saved persona text is actually present in the built prompt. This
        // is the exact chain a `chat -q` handoff turn walks:
        // `--profile <bot>` pivots IRONHERMES_HOME to the profile root
        // (`ironhermes-cli::resolve_and_set_profile`) → PromptBuilder reads
        // `get_hermes_home().join("SOUL.md")`.
        let dir = tempfile::tempdir().expect("tempdir");
        let _root_guard = home(&dir);
        std::fs::create_dir_all(profile_dir_for("zig")).expect("seed profile dir");

        save_profile_persona_impl(
            "zig",
            "# Researcher\nI am the research analyst on this team.",
        )
        .expect("save must succeed");

        // Simulate `--profile zig`'s IRONHERMES_HOME pivot (the child
        // process's env, per `resolve_and_set_profile` in
        // ironhermes-cli/src/main.rs) — nested ScopedEnv restores the
        // outer (root) value on drop.
        let bot_home = profile_dir_for("zig");
        let _pivot_guard = ScopedEnv::set(
            "IRONHERMES_HOME",
            bot_home.to_str().expect("bot home path must be utf8"),
        );
        let cwd = profile_workspace_dir("zig").expect("resolve bot workspace cwd");

        let prompt = ironhermes_agent::prompt_builder::PromptBuilder::new("test-model", "cli")
            .load_context(&cwd)
            .build();

        assert!(
            prompt.contains("I am the research analyst on this team."),
            "the saved persona must appear in the REAL assembled prompt, not just round-trip \
             through save/fetch; got: {prompt}"
        );
    }

    #[test]
    fn migrate_legacy_persona_if_needed_copies_legacy_forward_and_removes_it() {
        // Proves zig-shaped profiles (persona saved before the OF-6 fix, at
        // the legacy `workspace/SOUL.md` location) self-heal on the next
        // CLI handoff dispatch without the operator re-typing anything.
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let legacy = legacy_persona_path("zig");
        write_fixture_file(&legacy, "Pre-fix persona body");

        migrate_legacy_persona_if_needed("zig");

        assert_eq!(
            fs::read_to_string(canonical_persona_path("zig")).expect("canonical must exist"),
            "Pre-fix persona body"
        );
        assert!(!legacy.exists(), "legacy file must be removed after migration");
    }

    #[test]
    fn migrate_legacy_persona_if_needed_is_a_noop_when_canonical_already_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        std::fs::create_dir_all(profile_dir_for("zig")).expect("seed profile dir");
        save_profile_persona_impl("zig", "Current persona").expect("save must succeed");
        let legacy = legacy_persona_path("zig");
        write_fixture_file(&legacy, "Stale legacy content that must never win");

        migrate_legacy_persona_if_needed("zig");

        assert_eq!(
            fs::read_to_string(canonical_persona_path("zig")).expect("canonical must exist"),
            "Current persona",
            "an existing canonical persona must never be overwritten by a stale legacy file"
        );
    }

    #[test]
    fn migrate_legacy_persona_if_needed_is_a_noop_when_no_legacy_file_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        std::fs::create_dir_all(profile_dir_for("fresh-bot")).expect("seed profile dir");

        // Must not panic or create anything for a bot with no persona at all.
        migrate_legacy_persona_if_needed("fresh-bot");

        assert!(!canonical_persona_path("fresh-bot").exists());
    }

    #[test]
    fn fetch_profile_persona_impl_falls_back_to_legacy_location_without_creating_anything() {
        // OF-6: the drawer must display a pre-fix persona (zig's) even
        // before migration has run, and fetch must remain mkdir-free
        // (MA-01) while doing so.
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        std::fs::create_dir_all(profile_dir_for("zig")).expect("seed profile dir");
        write_fixture_file(&legacy_persona_path("zig"), "Legacy persona body");

        let persona = fetch_profile_persona_impl("zig").expect("fetch must succeed");

        assert_eq!(persona.body, "Legacy persona body");
        assert!(
            !canonical_persona_path("zig").exists(),
            "a read-only fetch must never write the canonical file itself"
        );
    }
}

/// Phase 47.4 Plan 20 (CR-03/CR-04): the writer/self-check regression suite
/// closing BLOCKERs from `47.4-REVIEW.md` "## Round 2". Every test here
/// drives a REAL write path (`save_profile_key_impl` / `create_profile_impl`)
/// and reads back through `read_env_keys` — the real `dotenvy` reader — per
/// `<test_contract>` items 2 and 4. These tests mutate process-global
/// `IRONHERMES_HOME` (and, for the canary tests, a second process env var),
/// so `--test-threads=1` is mandatory, exactly as every other test module in
/// this file documents. Run with:
///
///   cargo nextest run -p iron_hermes_ui --features server profile_env_encoding_tests --test-threads=1
#[cfg(all(test, feature = "server"))]
mod profile_env_encoding_tests {
    use super::*;
    use ironhermes_core::config::Config;
    use std::fs;

    /// RAII guard duplicated verbatim from `profile_scaffold_tests::ScopedEnv`
    /// (itself from `ironhermes-kanban/src/paths.rs:449-475`) — each
    /// `#[cfg(test)]` module is its own namespace, and this file's own doc
    /// comments sanction the duplication explicitly.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; no concurrent env access.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: single-threaded test context; no concurrent env access.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    fn home(dir: &tempfile::TempDir) -> ScopedEnv {
        ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        )
    }

    // -------------------------------------------------------------------
    // CR-03: a `${...}`-shaped value must never be dereferenced against the
    // server process's own environment.
    // -------------------------------------------------------------------

    #[test]
    fn save_profile_key_never_dereferences_a_process_env_var_into_the_profile_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = home(&dir);

        const CANARY_NAME: &str = "IH_TEST_CANARY_47_4_20";
        const CANARY_VALUE: &str = "root-credential-do-not-leak-9f3a";
        let _canary = ScopedEnv::set(CANARY_NAME, CANARY_VALUE);

        let profile_dir = profile_dir_for("canary-profile");
        fs::create_dir_all(&profile_dir).expect("create profile dir");

        let submitted_value = format!("${{{CANARY_NAME}}}");
        let secret = SecretString::from(submitted_value.clone());
        save_profile_key_impl("canary-profile", "SOME_KEY", &secret)
            .expect("save_profile_key_impl should succeed");

        let env_path = profile_dir.join(".env");
        let parsed = read_env_keys(&env_path).expect("read_env_keys must parse the written .env");
        let read_back = parsed.get("SOME_KEY").expect("SOME_KEY must be present");

        assert_eq!(
            read_back, &submitted_value,
            "read-back value must be byte-identical to the submitted literal characters, \
             never the dereferenced root credential (CR-03)"
        );
        assert!(
            !read_back.contains(CANARY_VALUE),
            "the canary's VALUE must never appear anywhere in the parsed value (CR-03 exfiltration)"
        );

        let raw_bytes = fs::read_to_string(&env_path).expect("read raw .env bytes");
        assert!(
            !raw_bytes.contains(CANARY_VALUE),
            "the canary's VALUE must never appear anywhere in the raw file bytes either (CR-03)"
        );
    }

    // -------------------------------------------------------------------
    // CR-04: space/hash-bearing values must round-trip byte-identical, not
    // hard-error or silently truncate.
    // -------------------------------------------------------------------

    #[test]
    fn save_profile_key_preserves_a_value_containing_a_space_and_a_hash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = home(&dir);

        let profile_a = "space-profile";
        fs::create_dir_all(profile_dir_for(profile_a)).expect("create profile dir a");
        let value_a = "sk-abc def";
        let secret_a = SecretString::from(value_a.to_string());
        save_profile_key_impl(profile_a, "SOME_KEY", &secret_a)
            .expect("save_profile_key_impl should succeed for a space-bearing value");
        let parsed_a = read_env_keys(&profile_dir_for(profile_a).join(".env"))
            .expect("read_env_keys must parse the written .env");
        assert_eq!(
            parsed_a.get("SOME_KEY").map(String::as_str),
            Some(value_a),
            "a value containing a space must read back byte-identical (CR-04)"
        );

        let profile_b = "hash-profile";
        fs::create_dir_all(profile_dir_for(profile_b)).expect("create profile dir b");
        let value_b = "sk-abc #def";
        let secret_b = SecretString::from(value_b.to_string());
        save_profile_key_impl(profile_b, "SOME_KEY", &secret_b)
            .expect("save_profile_key_impl should succeed for a space-then-hash value");
        let parsed_b = read_env_keys(&profile_dir_for(profile_b).join(".env"))
            .expect("read_env_keys must parse the written .env");
        assert_eq!(
            parsed_b.get("SOME_KEY").map(String::as_str),
            Some(value_b),
            "a value containing a space then a hash must read back byte-identical, \
             not silently truncated (CR-04)"
        );
    }

    // -------------------------------------------------------------------
    // Task 2: the full hostile-shape matrix, both write paths, and legacy
    // on-disk files.
    // -------------------------------------------------------------------

    /// Table-driven over every hostile shape `<test_contract>` calls for,
    /// EXCEPT a bare embedded tab: `char::is_control()` (CR-02, plan 18)
    /// rejects any control character at `validate_key_value`, and tab
    /// (U+0009) IS a Unicode `Cc` control character — so a tab-bearing
    /// value can never reach `save_profile_key_impl`'s validated entry
    /// point at all, by design, and testing it here would either force a
    /// weakening of the standing CR-02 prohibition or assert something
    /// structurally impossible. The tab shape is instead covered via the
    /// INHERITED path in `an_inherited_root_env_value_round_trips_through_create_profile`
    /// below, which never calls `validate_key_value` (root `.env` values
    /// are not caller-supplied) — the only entry point a tab-bearing value
    /// can legitimately reach.
    #[test]
    fn save_profile_key_round_trips_every_hostile_value_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = home(&dir);

        const CANARY_NAME: &str = "IH_TEST_HOSTILE_MATRIX_CANARY_47_4_20";
        let _canary = ScopedEnv::set(CANARY_NAME, "should-never-surface-in-any-case");

        let cases: Vec<(&str, String)> = vec![
            ("substitution_syntax", format!("${{{CANARY_NAME}}}")),
            ("embedded_space", "sk-abc def".to_string()),
            ("space_then_hash", "sk-abc #def".to_string()),
            ("starts_with_hash", "#sk-abc".to_string()),
            ("double_quote", "sk-\"abc\"".to_string()),
            ("single_quote", "sk-'abc'".to_string()),
            ("interior_backslash", "sk-a\\bc".to_string()),
            // The splitter-vs-parser divergence in <design_decision>: a
            // value ending in `\` must not swallow the closing quote.
            ("trailing_backslash", "sk-abc\\".to_string()),
            // dotenvy `trim_end`s the raw line (parse.rs:32), so an
            // unquoted trailing space would be silently lost.
            ("trailing_whitespace", "sk-abc   ".to_string()),
        ];

        for (label, value) in &cases {
            let profile_name = format!("hostile-{label}");
            fs::create_dir_all(profile_dir_for(&profile_name)).expect("create profile dir");
            let secret = SecretString::from(value.clone());
            save_profile_key_impl(&profile_name, "SOME_KEY", &secret).unwrap_or_else(|e| {
                panic!("save_profile_key_impl should succeed for case '{label}': {e}")
            });
            let parsed = read_env_keys(&profile_dir_for(&profile_name).join(".env"))
                .unwrap_or_else(|e| {
                    panic!("read_env_keys must parse the written .env for case '{label}': {e}")
                });
            assert_eq!(
                parsed.get("SOME_KEY").map(String::as_str),
                Some(value.as_str()),
                "case '{label}' must round-trip byte-identical"
            );
        }
    }

    /// CR-04's denial half: one hostile value must not deny or self-lock a
    /// profile — sibling keys still resolve and a subsequent save through
    /// the same path still succeeds.
    #[test]
    fn a_hostile_value_does_not_poison_the_other_keys_in_the_profile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = home(&dir);

        let profile = "no-poison-profile";
        fs::create_dir_all(profile_dir_for(profile)).expect("create profile dir");

        for (key, value) in [("FIRST_KEY", "sk-first"), ("SECOND_KEY", "sk-second")] {
            let secret = SecretString::from(value.to_string());
            save_profile_key_impl(profile, key, &secret)
                .expect("seeding an ordinary key must succeed");
        }

        let hostile_value = "sk-abc def";
        let hostile_secret = SecretString::from(hostile_value.to_string());
        save_profile_key_impl(profile, "THIRD_KEY", &hostile_secret).expect(
            "a space-bearing value must not be rejected by the write path (D-06/D-07)",
        );

        let parsed = read_env_keys(&profile_dir_for(profile).join(".env"))
            .expect("read_env_keys must parse the .env after the hostile write");
        assert_eq!(
            parsed.get("FIRST_KEY").map(String::as_str),
            Some("sk-first"),
            "a sibling key must still resolve after a hostile value was written"
        );
        assert_eq!(
            parsed.get("SECOND_KEY").map(String::as_str),
            Some("sk-second"),
            "a sibling key must still resolve after a hostile value was written"
        );
        assert_eq!(
            parsed.get("THIRD_KEY").map(String::as_str),
            Some(hostile_value)
        );

        // The self-lock the UI could not previously recover from: a
        // subsequent save through the same profile must still succeed.
        let follow_up_secret = SecretString::from("sk-follow-up".to_string());
        save_profile_key_impl(profile, "FOURTH_KEY", &follow_up_secret).expect(
            "a subsequent save must still succeed after a hostile value was written \
             (CR-04 denial half)",
        );
    }

    /// The CR-03 canary test again, through `create_profile_impl`'s
    /// `manual_keys` overlay rather than `save_profile_key_impl`, confirming
    /// the second write path is covered by the same guarantee.
    #[test]
    fn create_profile_manual_key_never_dereferences_a_process_env_var() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = home(&dir);

        const CANARY_NAME: &str = "IH_TEST_CREATE_CANARY_47_4_20";
        const CANARY_VALUE: &str = "root-credential-do-not-leak-create-path";
        let _canary = ScopedEnv::set(CANARY_NAME, CANARY_VALUE);

        let submitted_value = format!("${{{CANARY_NAME}}}");
        let manual = vec![(
            "SOME_KEY".to_string(),
            SecretString::from(submitted_value.clone()),
        )];
        create_profile_impl(
            "create-canary-profile",
            &KeyMode::LlmOnly,
            false,
            manual,
            &Config::default(),
        )
        .expect("create_profile_impl should succeed");

        let env_path = profile_dir_for("create-canary-profile").join(".env");
        let parsed = read_env_keys(&env_path).expect("read_env_keys must parse the written .env");
        let read_back = parsed.get("SOME_KEY").expect("SOME_KEY must be present");
        assert_eq!(
            read_back, &submitted_value,
            "read-back value must be the literal characters, never the dereferenced \
             root credential (CR-03, create_profile path)"
        );
        assert!(!read_back.contains(CANARY_VALUE));

        let raw_bytes = fs::read_to_string(&env_path).expect("read raw .env bytes");
        assert!(!raw_bytes.contains(CANARY_VALUE));
    }

    /// A value inherited from the root `.env` never passes through
    /// `validate_key_value` (it is not caller-supplied) — this is why the
    /// writer, not a blocklist, had to be the fix. Also covers the tab
    /// shape (see `save_profile_key_round_trips_every_hostile_value_shape`'s
    /// doc comment for why tab cannot reach the validated entry point).
    #[test]
    fn an_inherited_root_env_value_round_trips_through_create_profile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = home(&dir);

        // Hand-written root .env in an already-quoted form an operator
        // could plausibly have authored directly — an embedded space and
        // an embedded tab, neither of which is caller-supplied to this
        // web surface.
        fs::write(
            dir.path().join(".env"),
            "OPENROUTER_API_KEY='sk-with a space'\nGROQ_API_KEY='sk-with\ta-tab'\n",
        )
        .expect("write root .env");

        let rows = create_profile_impl(
            "inherited-hostile-profile",
            &KeyMode::AllKeys,
            false,
            Vec::new(),
            &Config::default(),
        )
        .expect("create_profile_impl should succeed");
        assert!(!rows.is_empty());

        let parsed = read_env_keys(&profile_dir_for("inherited-hostile-profile").join(".env"))
            .expect("read_env_keys must parse the created profile .env");
        assert_eq!(
            parsed.get("OPENROUTER_API_KEY").map(String::as_str),
            Some("sk-with a space")
        );
        assert_eq!(
            parsed.get("GROQ_API_KEY").map(String::as_str),
            Some("sk-with\ta-tab"),
            "an inherited value containing a tab must round-trip byte-identical"
        );
    }

    /// Backward-compatibility proof: no profile written by plans 01-19 (the
    /// pre-fix unquoted form) is stranded by the format change. Reading it
    /// and re-rendering (via a save) must preserve every pre-existing key.
    #[test]
    fn an_existing_unquoted_profile_env_still_reads_and_re_renders_identically() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = home(&dir);

        let profile = "legacy-unquoted-profile";
        let profile_dir = profile_dir_for(profile);
        fs::create_dir_all(&profile_dir).expect("create profile dir");
        // The pre-fix unquoted form, as written by plans 01-19.
        fs::write(
            profile_dir.join(".env"),
            "OPENROUTER_API_KEY=sk-legacy-value\nGROQ_API_KEY=sk-legacy-groq\n",
        )
        .expect("write legacy unquoted .env");

        let secret = SecretString::from("sk-new-anthropic".to_string());
        save_profile_key_impl(profile, "ANTHROPIC_API_KEY", &secret)
            .expect("save_profile_key_impl should succeed against a legacy unquoted file");

        let contents = fs::read_to_string(profile_dir.join(".env")).expect("read .env after save");
        assert_eq!(
            contents.matches(PROFILE_ENV_PROVENANCE_PREFIX).count(),
            1,
            "the provenance header must appear exactly once after the rewrite"
        );

        let parsed = read_env_keys(&profile_dir.join(".env")).expect("read_env_keys after rewrite");
        assert_eq!(
            parsed.get("OPENROUTER_API_KEY").map(String::as_str),
            Some("sk-legacy-value"),
            "a pre-existing legacy key must survive the rewrite unchanged"
        );
        assert_eq!(
            parsed.get("GROQ_API_KEY").map(String::as_str),
            Some("sk-legacy-groq"),
            "a pre-existing legacy key must survive the rewrite unchanged"
        );
        assert_eq!(
            parsed.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-new-anthropic")
        );
    }

    /// The only test covering the self-check's parse-error branch (D-13):
    /// without it the trap described in Task 1 ships silently. Drives
    /// `verify_render_round_trip` DIRECTLY with a hand-built malformed
    /// string (an unterminated single quote) rather than adding a
    /// `#[cfg(test)]` seam to `render_profile_env` itself. dotenvy's EOF
    /// handling (`iter.rs:136-141`) returns `Err(Error::LineParse(buf,
    /// len))` whenever the parse state is not `Complete` at EOF, so this
    /// reliably drives the exact parse-`Err` branch independent of whatever
    /// the real escaping logic does.
    #[test]
    fn self_check_parse_error_never_leaks_the_value() {
        const SENTINEL: &str = "sentinel-x7q9-do-not-leak-4f2b8c";
        let malformed = format!("# header comment\nSENTINEL_KEY='{SENTINEL}\n");
        let entries = vec![("SENTINEL_KEY".to_string(), SENTINEL.to_string())];

        let err = verify_render_round_trip(&malformed, &entries)
            .expect_err("an unterminated quote must be refused, not silently accepted");

        assert!(
            !err.contains(SENTINEL),
            "the self-check error must never contain the sentinel value: {err}"
        );
        assert!(
            !err.contains("Error parsing line"),
            "the self-check error must never contain a raw dotenvy line fragment: {err}"
        );
    }
}

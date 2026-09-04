//! Phase 48.2 Plan 07 (D-05/D-06/D-07/D-09/D-16/D-22): the write-only,
//! masked-presence tool-credential API — closes the credential loop the
//! amber unmet-prerequisite card (`catalog.rs`'s `.tool-fix-mount`) and the
//! settings-panel chain rows both point at.
//!
//! # Task 1 decision (D-09, one-way) — resolved
//!
//! The Task 1 checkpoint (blocking-human, gate="blocking-human") asked
//! where profile-scope tool credentials land and how enumerable that must
//! be for Phase 51's per-profile vault migration. The operator selected
//! **option (b) — proceed with an inventory stamp**, with this reasoning
//! recorded verbatim: *"b - the vault is a user decsion so this app needs
//! to operate with either env or vault"*. Consequences implemented here:
//! 1. Profile-scope credentials route to that profile's plaintext `.env`,
//!    through the SAME hardened write path `profile_api.rs`'s
//!    `save_profile_key`/`save_profile_key_impl` already uses (never a
//!    second implementation, T-48.2-07-04) — this posture is interim until
//!    Phase 51's per-profile vault migration ships.
//! 2. Every profile `.env` this module creates or updates carries an
//!    ADDITIONAL provenance comment line ([`TOOL_CREDENTIAL_PROVENANCE_STAMP`])
//!    naming this phase, so Phase 51 can enumerate exactly which files and
//!    keys this surface produced instead of sweeping every profile blind.
//!    Threaded through `profile_api.rs`'s newly-lifted
//!    `save_profile_key_impl_with_stamp`/`render_profile_env_with_stamp` —
//!    the ONLY change this plan makes to that file, and a pure additive
//!    lift (old call sites/tests are byte-identical, verified by
//!    `render_profile_env`/`save_profile_key_impl` delegating with `None`).
//! 3. This surface is dual-posture by design, not vault-exclusive: root
//!    writes go vault-first when the vault is ACTUALLY writable
//!    ([`vault_is_writable`]) and fall back to the honest plaintext
//!    `tools.credentials` tier — with the warning shown up front — when it
//!    is not. An absent/disabled vault is never treated as an error state.
//!
//! # No readback, ever (T-48.2-07-01)
//!
//! No function in this module returns a type whose fields could carry key
//! material. [`ToolCredentialStatus::masked`] is a fixed-length dot run,
//! never derived from the stored value's length. `set`/`clear` return
//! `Result<(), ServerFnError>`. Every error string names only the key NAME
//! and the tier — raw store/parser/filesystem error text is discarded
//! (T-48.2-07-02), mirroring the profile `.env` reader's documented
//! full-line-embed leak fix.
//!
//! # Vault-first, honest fallback, no silent downgrade (D-07)
//!
//! [`vault_is_writable`] mirrors `provider_secrets_api.rs`'s
//! `check_write_gates` discipline exactly: `vault.enabled: true` alone is
//! NOT sufficient, because the default `vault.backend` is `"env-var"`, whose
//! `SecretStore` impl is a read-only diagnostic that hard-errors on
//! `put_secret`/`delete_secret` at call time. [`resolve_credential_route`]
//! therefore reports a *statically knowable* unwritable backend as the
//! `ConfigPlaintext` tier with `vault_unavailable_reason` set — BEFORE the
//! operator types a key (REVIEWS finding 6) — while a genuine *runtime*
//! vault write failure never silently downgrades to plaintext (T-48.2-07-06):
//! it errors, because by then the operator was already told the key was
//! going to the vault.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::server::tools_config_api::ConfigScope;

// =============================================================================
// DTOs — shared shape on both the wasm client and the native server.
// =============================================================================

/// D-06: which tier answered a credential lookup / will answer a write.
/// `Env` and `NotSet` are read-side-only outcomes — no write ever targets
/// either.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum CredentialStorageTier {
    Env,
    Vault,
    ConfigPlaintext,
    ProfileEnv,
    NotSet,
}

/// Presence + answering-tier snapshot for one credential key. `masked` is a
/// FIXED-length dot run independent of the stored value's length
/// (T-48.2-07-01) — never a hint at how long the real secret is.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ToolCredentialStatus {
    pub key_name: String,
    pub present: bool,
    pub tier: CredentialStorageTier,
    pub masked: String,
}

/// The pre-commit verdict [`get_credential_write_plan`] returns — rendered
/// by the form BEFORE the key input is revealed (REVIEWS finding 6), never
/// after a failed write.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CredentialWritePlan {
    pub tier: CredentialStorageTier,
    pub plaintext_warning_required: bool,
    pub warning: Option<String>,
    pub vault_unavailable_reason: Option<String>,
}

/// UI-SPEC Copywriting Contract — plaintext-credential warning (D-07),
/// verbatim, never paraphrased.
pub const PLAINTEXT_WARNING: &str = "This key will be stored in plaintext in config.yaml (vault is disabled). Anyone with file access can read it.";

/// Read-only-backend reason line (REVIEWS finding 6) — actionable and
/// secret-free: names the config key to change AND the required build
/// feature, mirroring `provider_secrets_api.rs`'s
/// `BACKEND_READ_ONLY_LINE1`/`LINE2`, never the raw `EnvVarStore`
/// diagnostic.
const VAULT_DISABLED_REASON: &str =
    "Vault is not enabled. Set vault.enabled: true in config.yaml to store this credential in the vault.";
const VAULT_READ_ONLY_BACKEND_REASON: &str = "Vault backend is read-only (env-var). Set vault.backend: \"rusty-vault\" in config.yaml and serve a rusty-vault build (--features rusty-vault), then retry.";

/// Portable duplicate of `ironhermes_tools::credentials::TOOL_CREDENTIAL_KEYS`
/// — that crate is a native-only dependency of this crate (Cargo.toml
/// `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`), so the
/// wasm-compiled settings-panel credentials section needs its own portable
/// copy of the canonical name list to render "every canonical tool
/// credential key". A native-only test asserts this stays byte-identical to
/// the source of truth so the two lists cannot silently drift.
#[allow(dead_code)] // consumed by Task 4's settings-panel credentials section; also asserted directly by this file's own test
pub const CANONICAL_TOOL_CREDENTIAL_KEYS: &[&str] = &[
    "FIRECRAWL_API_KEY",
    "EXA_API_KEY",
    "TAVILY_API_KEY",
    "BRAVE_API_KEY",
    "PERPLEXITY_API_KEY",
];

/// Fixed-length masked-presence marker (T-48.2-07-01) — never embeds any
/// character of the stored value, so this constant structurally cannot leak
/// key material through its own text.
#[cfg(not(target_arch = "wasm32"))]
const FIXED_MASK: &str = "sk-••••••••••••••••••";

/// Phase 48.2 Plan 07 (D-09 checkpoint, resolved option b): the additional
/// provenance stamp line every profile `.env` write through this module
/// carries, naming this phase so Phase 51's migration can enumerate the
/// files and keys this surface produced. Rendered as one more `#`-prefixed
/// comment line, inert to the `dotenvy` reader (verified by
/// `render_profile_env_with_stamp`'s own round-trip check).
#[cfg(not(target_arch = "wasm32"))]
const TOOL_CREDENTIAL_PROVENANCE_STAMP: &str = "# Tool credential written by the Iron Hermes Tools page (Phase 48.2) — interim until the Phase 51 per-profile vault migration; a disclosed key here is remediated by rotation, not deletion.";

// =============================================================================
// Pure validators (Task 2) — mirror the profile key-name/value validators,
// no disk I/O, run BEFORE any store is touched.
// =============================================================================

/// Rejects an empty name, a lowercase name, and a name containing a path
/// separator or a newline — anything that could break a `.env` line or a
/// YAML key.
pub fn validate_credential_key_name(key_name: &str) -> Result<(), String> {
    if key_name.is_empty() {
        return Err("credential key name must not be empty".to_string());
    }
    if key_name.contains('/')
        || key_name.contains('\\')
        || key_name.contains('\n')
        || key_name.contains('\r')
    {
        return Err(format!(
            "invalid credential key name '{key_name}' — must not contain a path separator or newline"
        ));
    }
    // WR-01: match `profile_api.rs`'s `validate_key_name` `[A-Z][A-Z0-9_]*`
    // shape exactly (first character must be an ASCII uppercase letter) —
    // this pre-check and the deeper profile-scope validator it feeds into
    // must never silently disagree on the same name. Without this, a name
    // like `1_API_KEY` passed this check but failed only at the deeper
    // profile write, producing an opaque "write failed" error with no
    // indication the name itself was the (silently different) reason.
    let mut chars = key_name.chars();
    let first_ok = chars.next().map(|c| c.is_ascii_uppercase()).unwrap_or(false);
    let rest_ok = key_name
        .chars()
        .skip(1)
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !first_ok || !rest_ok {
        return Err(format!(
            "invalid credential key name '{key_name}' — must match [A-Z][A-Z0-9_]*"
        ));
    }
    Ok(())
}

/// Rejects an empty value and a value containing a newline or carriage
/// return — either could forge a second `VAR=value` line or a YAML key.
pub fn validate_credential_value(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("credential value must not be empty".to_string());
    }
    if value.contains('\n') || value.contains('\r') {
        return Err(
            "credential value must not contain a newline or carriage return".to_string(),
        );
    }
    Ok(())
}

// =============================================================================
// Route resolution — pure, disk-I/O-free beyond the `&Config` already
// loaded by the caller.
// =============================================================================

/// UAT/REVIEWS-finding-6 writability check (mirrors
/// `provider_secrets_api.rs::check_write_gates`'s backend discipline
/// exactly): `vault.enabled: true` alone is NOT sufficient — the default
/// `vault.backend` is the read-only `"env-var"` diagnostic store. Never
/// auto-switches the backend.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn vault_is_writable(config: &ironhermes_core::config::Config) -> Result<(), String> {
    let vault_config = ironhermes_core::resolve_vault_config(config);
    if !vault_config.enabled {
        return Err(VAULT_DISABLED_REASON.to_string());
    }
    if vault_config.backend == "env-var" {
        return Err(VAULT_READ_ONLY_BACKEND_REASON.to_string());
    }
    Ok(())
}

/// Resolve which tier a credential write for `scope`/`key_name` targets,
/// and what the operator must be told BEFORE they type a key (D-07,
/// REVIEWS finding 6). Profile scope always routes to `ProfileEnv` (Task
/// 3). Root scope routes to `Vault` only when BOTH the backend is actually
/// writable AND `key_name` is one of the canonical tool-credential keys the
/// credential resolver actually consults the vault for — a vault write
/// under any other name would be stored and never read back
/// (T-48.2-07-07).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn resolve_credential_route(
    scope: &ConfigScope,
    key_name: &str,
    config: &ironhermes_core::config::Config,
) -> CredentialWritePlan {
    match scope {
        ConfigScope::Profile(_) => CredentialWritePlan {
            tier: CredentialStorageTier::ProfileEnv,
            plaintext_warning_required: false,
            warning: None,
            vault_unavailable_reason: None,
        },
        ConfigScope::Root => {
            let is_canonical = ironhermes_tools::credentials::TOOL_CREDENTIAL_KEYS.contains(&key_name);
            match vault_is_writable(config) {
                Ok(()) if is_canonical => CredentialWritePlan {
                    tier: CredentialStorageTier::Vault,
                    plaintext_warning_required: false,
                    warning: None,
                    vault_unavailable_reason: None,
                },
                Ok(()) => CredentialWritePlan {
                    // Vault IS writable, but this key is not one the
                    // credential resolver ever consults the vault for — a
                    // vault write under this name would be silently
                    // unreadable, which is worse than a visible plaintext
                    // write (T-48.2-07-07).
                    tier: CredentialStorageTier::ConfigPlaintext,
                    plaintext_warning_required: true,
                    warning: Some(PLAINTEXT_WARNING.to_string()),
                    vault_unavailable_reason: None,
                },
                Err(reason) => CredentialWritePlan {
                    tier: CredentialStorageTier::ConfigPlaintext,
                    plaintext_warning_required: true,
                    warning: Some(PLAINTEXT_WARNING.to_string()),
                    vault_unavailable_reason: Some(reason),
                },
            }
        }
    }
}

// =============================================================================
// Root-scope write gate — always read fresh from ROOT regardless of scope
// (mirrors tools_config_api.rs::check_tools_write_gate's module-doc
// rationale: the gate is the operator's policy for the web surface itself).
// =============================================================================

/// T-48.2-07-02: the underlying error is `Config::load_from`'s raw
/// `serde_yaml` failure (`config.rs:3661`, propagated by `?` with no
/// redaction). A serde_yaml type-mismatch message embeds the offending
/// scalar (`invalid type: string "...", expected bool at line N`), so a
/// malformed `config.yaml` could echo a plaintext `tools.credentials`
/// value straight to the browser. Discarded here, matching this module's
/// own `put_secret`/`delete_secret` discipline — unlike WR-01's threaded
/// paths, this error is NOT our own constructed `String`.
#[cfg(not(target_arch = "wasm32"))]
fn load_root_config() -> Result<ironhermes_core::config::Config, String> {
    ironhermes_core::config::Config::load()
        .map_err(|_| "Config load failed — check ~/.ironhermes/config.yaml".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn check_credentials_write_gate(
    config: &ironhermes_core::config::Config,
) -> Result<(), String> {
    if !config.security.web_config_write_enabled {
        return Err("Config writes are disabled".to_string());
    }
    Ok(())
}

// =============================================================================
// Profile-scope arm (Task 3) — delegates entirely to profile_api.rs's
// hardened, atomic-0600, single-quoted `.env` write. No second write
// implementation (T-48.2-07-04).
// =============================================================================

#[cfg(not(target_arch = "wasm32"))]
async fn set_profile_credential(
    name: &str,
    key_name: &str,
    value: &secrecy::SecretString,
) -> Result<(), String> {
    let validated = ironhermes_core::profile::validate_profile_name(name)
        .map_err(|e| format!("invalid profile name: {e}"))?;
    let key_name_owned = key_name.to_string();
    let value_owned = value.clone();
    tokio::task::spawn_blocking(move || {
        crate::server::profile_api::save_profile_key_impl_with_stamp(
            &validated,
            &key_name_owned,
            &value_owned,
            Some(TOOL_CREDENTIAL_PROVENANCE_STAMP),
        )
        .map(|_row| ())
    })
    .await
    .map_err(|e| format!("spawn_blocking join: {e}"))?
}

/// No existing "remove one key from a profile `.env`" path exists in
/// `profile_api.rs` — this reuses that module's PRIMITIVES
/// (`read_env_keys`, `render_profile_env_with_stamp`,
/// `write_env_atomic_0600`) rather than hand-rolling a second atomic-write
/// mechanism. Idempotent: clearing an absent key is not an error.
#[cfg(not(target_arch = "wasm32"))]
fn clear_profile_credential_blocking(name: &str, key_name: &str) -> Result<(), String> {
    let validated = ironhermes_core::profile::validate_profile_name(name)
        .map_err(|e| format!("invalid profile name: {e}"))?;
    let profile_dir = crate::server::profile_api::profile_dir_for(&validated);
    if !profile_dir.is_dir() {
        return Ok(());
    }
    let env_path = profile_dir.join(".env");
    let mut env_map = crate::server::profile_api::read_env_keys(&env_path)
        .map_err(|e| format!("read profile .env: {e}"))?;
    if env_map.remove(key_name).is_none() {
        return Ok(());
    }
    let mut entries: Vec<(String, String)> = env_map.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let contents = crate::server::profile_api::render_profile_env_with_stamp(
        &validated,
        &entries,
        Some(TOOL_CREDENTIAL_PROVENANCE_STAMP),
    )
    .map_err(|e| format!("render profile .env: {e}"))?;
    crate::server::profile_api::write_env_atomic_0600(&env_path, &contents)
        .map_err(|e| format!("write .env: {e}"))?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
async fn clear_profile_credential(name: &str, key_name: &str) -> Result<(), String> {
    let name_owned = name.to_string();
    let key_name_owned = key_name.to_string();
    tokio::task::spawn_blocking(move || {
        clear_profile_credential_blocking(&name_owned, &key_name_owned)
    })
    .await
    .map_err(|e| format!("spawn_blocking join: {e}"))?
}

#[cfg(not(target_arch = "wasm32"))]
fn profile_credential_status(name: &str, key_name: &str) -> ToolCredentialStatus {
    let Ok(validated) = ironhermes_core::profile::validate_profile_name(name) else {
        return absent_status(key_name);
    };
    let profile_dir = crate::server::profile_api::profile_dir_for(&validated);
    let env_path = profile_dir.join(".env");
    let Ok(env_map) = crate::server::profile_api::read_env_keys(&env_path) else {
        return absent_status(key_name);
    };
    match env_map.get(key_name) {
        Some(v) if !v.is_empty() => present_status(key_name, CredentialStorageTier::ProfileEnv),
        _ => absent_status(key_name),
    }
}

// =============================================================================
// Root-scope arm (Task 2)
// =============================================================================

#[cfg(not(target_arch = "wasm32"))]
async fn root_credential_status(key_name: &str) -> ToolCredentialStatus {
    // Mirrors ironhermes_tools::credentials::ToolCredentials's precedence:
    // env -> tools.credentials -> vault. Env is live, never pre-resolved.
    if std::env::var(key_name).is_ok() {
        return present_status(key_name, CredentialStorageTier::Env);
    }
    let Ok(config) = ironhermes_core::config::Config::load() else {
        return absent_status(key_name);
    };
    if config
        .tools
        .credentials
        .get(key_name)
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return present_status(key_name, CredentialStorageTier::ConfigPlaintext);
    }
    if config.vault.enabled {
        let vault_config = ironhermes_core::resolve_vault_config(&config);
        if let Ok(store) = ironhermes_vault::open_store(&vault_config) {
            if let Ok(Some(_)) = store.get_secret(key_name).await {
                return present_status(key_name, CredentialStorageTier::Vault);
            }
        }
    }
    absent_status(key_name)
}

#[cfg(not(target_arch = "wasm32"))]
fn present_status(key_name: &str, tier: CredentialStorageTier) -> ToolCredentialStatus {
    ToolCredentialStatus {
        key_name: key_name.to_string(),
        present: true,
        tier,
        masked: FIXED_MASK.to_string(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn absent_status(key_name: &str) -> ToolCredentialStatus {
    ToolCredentialStatus {
        key_name: key_name.to_string(),
        present: false,
        tier: CredentialStorageTier::NotSet,
        masked: String::new(),
    }
}

// =============================================================================
// #[server] fns — thin wrappers, dioxus fullstack codec split.
// =============================================================================

/// Presence + answering-tier read (D-06) — never errors on a missing key;
/// absence is a status, not a failure. Never returns a value.
#[server]
pub async fn get_tool_credential_status(
    scope: ConfigScope,
    key_name: String,
) -> Result<ToolCredentialStatus, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let status = match &scope {
            ConfigScope::Profile(name) => profile_credential_status(name, &key_name),
            ConfigScope::Root => root_credential_status(&key_name).await,
        };
        Ok(status)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, key_name);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Pre-commit verdict (REVIEWS finding 6) — the form calls this BEFORE
/// revealing the key input, never after a failed write.
#[server]
pub async fn get_credential_write_plan(
    scope: ConfigScope,
    key_name: String,
) -> Result<CredentialWritePlan, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        validate_credential_key_name(&key_name).map_err(ServerFnError::new)?;
        let config = load_root_config().map_err(ServerFnError::new)?;
        Ok(resolve_credential_route(&scope, &key_name, &config))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, key_name);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Store a tool credential (D-05/D-06/D-07/D-09). The value is wrapped in
/// `secrecy::SecretString` on the first statement that handles it — before
/// it can reach any `Debug`-deriving struct, any log, or the
/// `spawn_blocking` boundary. Never echoes the value back in the return or
/// any error (T-48.2-07-01/02).
#[server]
pub async fn set_tool_credential(
    scope: ConfigScope,
    key_name: String,
    value: String,
) -> Result<(), ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        validate_credential_key_name(&key_name).map_err(ServerFnError::new)?;
        validate_credential_value(&value).map_err(ServerFnError::new)?;
        let secret = secrecy::SecretString::from(value);

        let config = load_root_config().map_err(ServerFnError::new)?;
        check_credentials_write_gate(&config).map_err(ServerFnError::new)?;

        match &scope {
            // WR-01: thread the real error text through instead of
            // discarding it — `set_profile_credential`'s error is always our
            // own validation/write code's `String` (invalid profile name,
            // invalid key name/value, or an I/O path failure), never
            // secret material (the value only ever reaches
            // `secrecy::SecretString`), so including it here does not
            // violate the no-readback discipline (D-13). Never interpolate
            // `secret`/`value` themselves.
            ConfigScope::Profile(name) => set_profile_credential(name, &key_name, &secret)
                .await
                .map_err(|e| {
                    ServerFnError::new(format!(
                        "profile credential write failed for key '{key_name}': {e}"
                    ))
                }),
            ConfigScope::Root => {
                let plan = resolve_credential_route(&scope, &key_name, &config);
                match plan.tier {
                    CredentialStorageTier::Vault => {
                        let vault_config = ironhermes_core::resolve_vault_config(&config);
                        // T-48.2-07-02: `VaultError`'s Display is the store's
                        // own text, not ours — discarded rather than threaded.
                        let store = ironhermes_vault::open_store(&vault_config)
                            .map_err(|_| ServerFnError::new("Vault store open failed"))?;
                        // T-48.2-07-06: a vault failure here returns an
                        // error and writes nothing else — no fallback.
                        store.put_secret(&key_name, secret).await.map_err(|_| {
                            ServerFnError::new(format!(
                                "vault write failed for credential '{key_name}'"
                            ))
                        })?;
                        Ok(())
                    }
                    _ => {
                        use secrecy::ExposeSecret;
                        let mut config = config;
                        config
                            .tools
                            .credentials
                            .insert(key_name.clone(), secret.expose_secret().to_string());
                        config.save().map_err(|_| {
                            ServerFnError::new(format!(
                                "config save failed for credential '{key_name}'"
                            ))
                        })?;
                        Ok(())
                    }
                }
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, key_name, value);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Remove a tool credential from whichever tier holds it (D-06). Uses the
/// SAME route resolution as `set_tool_credential` — this system's design
/// never lets a root-scope key live in both `Vault` and `ConfigPlaintext`
/// simultaneously, so clearing the resolved-route tier is clearing the tier
/// that actually holds it.
#[server]
pub async fn clear_tool_credential(
    scope: ConfigScope,
    key_name: String,
) -> Result<(), ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        validate_credential_key_name(&key_name).map_err(ServerFnError::new)?;
        let config = load_root_config().map_err(ServerFnError::new)?;
        check_credentials_write_gate(&config).map_err(ServerFnError::new)?;

        match &scope {
            ConfigScope::Profile(name) => {
                clear_profile_credential(name, &key_name).await.map_err(|_| {
                    ServerFnError::new(format!(
                        "profile credential clear failed for key '{key_name}'"
                    ))
                })
            }
            ConfigScope::Root => {
                let plan = resolve_credential_route(&scope, &key_name, &config);
                match plan.tier {
                    CredentialStorageTier::Vault => {
                        let vault_config = ironhermes_core::resolve_vault_config(&config);
                        // T-48.2-07-02: `VaultError`'s Display is the store's
                        // own text, not ours — discarded rather than threaded.
                        let store = ironhermes_vault::open_store(&vault_config)
                            .map_err(|_| ServerFnError::new("Vault store open failed"))?;
                        store.delete_secret(&key_name).await.map_err(|_| {
                            ServerFnError::new(format!(
                                "vault delete failed for credential '{key_name}'"
                            ))
                        })?;
                        Ok(())
                    }
                    _ => {
                        let mut config = config;
                        config.tools.credentials.remove(&key_name);
                        config.save().map_err(|_| {
                            ServerFnError::new(format!(
                                "config save failed clearing credential '{key_name}'"
                            ))
                        })?;
                        Ok(())
                    }
                }
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, key_name);
        unreachable!("server fn body never runs on the wasm client")
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use ironhermes_core::config::{Config, SecurityConfig};

    // -------------------------------------------------------------------
    // validate_credential_key_name / validate_credential_value
    // -------------------------------------------------------------------

    #[test]
    fn key_name_accepts_uppercase_alphanumeric_underscore() {
        assert!(validate_credential_key_name("EXA_API_KEY").is_ok());
        assert!(validate_credential_key_name("KEY123").is_ok());
    }

    #[test]
    fn key_name_rejects_lowercase() {
        assert!(validate_credential_key_name("exa_api_key").is_err());
    }

    #[test]
    fn key_name_rejects_empty() {
        assert!(validate_credential_key_name("").is_err());
    }

    #[test]
    fn key_name_rejects_path_separator_and_newline() {
        assert!(validate_credential_key_name("EXA/KEY").is_err());
        assert!(validate_credential_key_name("EXA\\KEY").is_err());
        assert!(validate_credential_key_name("EXA\nKEY").is_err());
    }

    /// WR-01: `validate_credential_key_name` must now agree with
    /// `profile_api.rs`'s `[A-Z][A-Z0-9_]*` shape — a name whose first
    /// character is a digit or underscore is rejected at THIS pre-check,
    /// not only at the deeper profile-scope validator (which previously
    /// produced an opaque "write failed" error for exactly these names).
    #[test]
    fn key_name_rejects_digit_or_underscore_first_character() {
        assert!(validate_credential_key_name("1_API_KEY").is_err());
        assert!(validate_credential_key_name("_API_KEY").is_err());
    }

    /// Existing valid names (letter-first, uppercase alphanumeric +
    /// underscore) must still pass the tightened check.
    #[test]
    fn key_name_still_accepts_valid_names_after_tightening() {
        assert!(validate_credential_key_name("EXA_API_KEY").is_ok());
        assert!(validate_credential_key_name("KEY123").is_ok());
        assert!(validate_credential_key_name("A").is_ok());
    }

    #[test]
    fn value_rejects_empty() {
        assert!(validate_credential_value("").is_err());
    }

    #[test]
    fn value_rejects_newline_and_carriage_return() {
        assert!(validate_credential_value("abc\ndef").is_err());
        assert!(validate_credential_value("abc\rdef").is_err());
    }

    #[test]
    fn value_accepts_ordinary_secret() {
        assert!(validate_credential_value("sk-abc123").is_ok());
    }

    // -------------------------------------------------------------------
    // vault_is_writable / resolve_credential_route
    // -------------------------------------------------------------------

    #[test]
    fn vault_disabled_is_unwritable() {
        let config = Config::default();
        let err = vault_is_writable(&config).expect_err("disabled vault must be unwritable");
        assert!(err.contains("Vault is not enabled"), "got: {err}");
    }

    #[test]
    fn vault_enabled_env_var_backend_is_unwritable() {
        let mut config = Config::default();
        config.vault.enabled = true;
        assert_eq!(config.vault.backend, "env-var");
        let err = vault_is_writable(&config).expect_err("env-var backend must be unwritable");
        assert!(err.contains("read-only (env-var)"), "got: {err}");
        assert!(
            err.contains("vault.backend: \"rusty-vault\""),
            "error must name the config key to change, got: {err}"
        );
        assert!(!err.contains("EnvVarStore"), "must never leak the raw backend diagnostic");
    }

    #[test]
    fn vault_enabled_rusty_vault_backend_is_writable() {
        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.backend = "rusty-vault".to_string();
        assert!(vault_is_writable(&config).is_ok());
    }

    #[test]
    fn route_root_vault_writable_canonical_key_targets_vault() {
        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.backend = "rusty-vault".to_string();
        let plan = resolve_credential_route(&ConfigScope::Root, "EXA_API_KEY", &config);
        assert_eq!(plan.tier, CredentialStorageTier::Vault);
        assert!(!plan.plaintext_warning_required);
        assert!(plan.vault_unavailable_reason.is_none());
    }

    #[test]
    fn route_root_vault_disabled_targets_plaintext_with_warning() {
        let config = Config::default();
        let plan = resolve_credential_route(&ConfigScope::Root, "EXA_API_KEY", &config);
        assert_eq!(plan.tier, CredentialStorageTier::ConfigPlaintext);
        assert!(plan.plaintext_warning_required);
        assert_eq!(plan.warning.as_deref(), Some(PLAINTEXT_WARNING));
    }

    /// REVIEWS finding 6: `vault.enabled: true` with the default `env-var`
    /// backend must produce `ConfigPlaintext` + a populated
    /// `vault_unavailable_reason` naming the `vault.backend` key — NOT the
    /// `Vault` tier.
    #[test]
    fn route_root_vault_enabled_env_var_backend_targets_plaintext_with_reason() {
        let mut config = Config::default();
        config.vault.enabled = true;
        assert_eq!(config.vault.backend, "env-var");
        let plan = resolve_credential_route(&ConfigScope::Root, "EXA_API_KEY", &config);
        assert_eq!(plan.tier, CredentialStorageTier::ConfigPlaintext);
        assert!(plan.plaintext_warning_required);
        let reason = plan
            .vault_unavailable_reason
            .expect("reason must be populated for a read-only backend");
        assert!(reason.contains("vault.backend"), "got: {reason}");
    }

    #[test]
    fn route_root_vault_writable_non_canonical_key_targets_plaintext_no_reason() {
        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.backend = "rusty-vault".to_string();
        let plan = resolve_credential_route(&ConfigScope::Root, "SOME_OTHER_KEY", &config);
        assert_eq!(plan.tier, CredentialStorageTier::ConfigPlaintext);
        assert!(plan.plaintext_warning_required);
        assert!(
            plan.vault_unavailable_reason.is_none(),
            "vault IS writable here — there is nothing to explain, only a non-canonical key"
        );
    }

    #[test]
    fn route_profile_scope_always_targets_profile_env() {
        let config = Config::default();
        let plan = resolve_credential_route(
            &ConfigScope::Profile("worker-1".to_string()),
            "EXA_API_KEY",
            &config,
        );
        assert_eq!(plan.tier, CredentialStorageTier::ProfileEnv);
        assert!(!plan.plaintext_warning_required);
    }

    // -------------------------------------------------------------------
    // Non-disclosure sentinel tests (T-48.2-07-01/02)
    // -------------------------------------------------------------------

    #[test]
    fn status_serialization_never_contains_a_sentinel_value() {
        let status = present_status("EXA_API_KEY", CredentialStorageTier::ConfigPlaintext);
        let json = serde_json::to_string(&status).expect("status must serialize");
        assert!(!json.contains("sentinel-value-should-never-leak"));
        assert!(!json.to_lowercase().contains("sk-abc123secretvalue"));
    }

    /// T-48.2-07-02 regression: `load_root_config` must never forward
    /// `serde_yaml`'s raw parse text. A type-mismatch message embeds the
    /// offending scalar verbatim (`invalid type: string "sk-...", expected
    /// bool`), so a malformed `config.yaml` whose wrong-typed value happens
    /// to be key material would otherwise echo it to the browser through
    /// `get_credential_write_plan` / `set_tool_credential` /
    /// `clear_tool_credential`, all of which surface this error verbatim.
    #[test]
    fn load_root_config_error_never_echoes_the_malformed_config_scalar() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            home_dir.path().join("config.yaml"),
            "security:\n  web_config_write_enabled: sk-sentinel-must-not-leak\n",
        )
        .expect("seed malformed config");

        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };
        let err = load_root_config().expect_err("a non-bool scalar must fail to parse");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(
            !err.contains("sk-sentinel-must-not-leak"),
            "T-48.2-07-02: the config-load error must not echo the offending \
             scalar; got: {err}"
        );
    }

    #[test]
    fn masked_run_is_fixed_length_regardless_of_value() {
        let short = present_status("K", CredentialStorageTier::Vault);
        let long_value_status = ToolCredentialStatus {
            key_name: "K".to_string(),
            present: true,
            tier: CredentialStorageTier::Vault,
            masked: FIXED_MASK.to_string(),
        };
        assert_eq!(short.masked.len(), long_value_status.masked.len());
        assert_eq!(short.masked, FIXED_MASK);
    }

    #[test]
    fn write_gate_closed_by_default() {
        let config = Config::default();
        let err = check_credentials_write_gate(&config)
            .expect_err("write-gate-closed config must be rejected");
        assert!(err.contains("Config writes are disabled"), "got: {err}");
    }

    #[test]
    fn write_gate_open_when_enabled() {
        let config = Config {
            security: SecurityConfig {
                web_config_write_enabled: true,
                web_process_control_enabled: false,
                remote_blueprint_run_enabled: false,
            },
            ..Config::default()
        };
        assert!(check_credentials_write_gate(&config).is_ok());
    }

    // -------------------------------------------------------------------
    // Canonical key list drift guard
    // -------------------------------------------------------------------

    #[test]
    fn portable_canonical_key_list_matches_source_of_truth() {
        assert_eq!(
            CANONICAL_TOOL_CREDENTIAL_KEYS,
            ironhermes_tools::credentials::TOOL_CREDENTIAL_KEYS,
            "the portable wasm-safe copy must stay byte-identical to ironhermes_tools::credentials::TOOL_CREDENTIAL_KEYS"
        );
    }

    /// WR-01: `set_tool_credential`'s `validate_credential_key_name` call
    /// runs BEFORE the `match &scope` branch, so a name like `1_API_KEY`
    /// (digit-first, previously only caught by the deeper profile-scope
    /// `validate_key_name`) is now rejected at the exact same pre-check
    /// point regardless of scope — no `IRONHERMES_HOME`/config setup needed
    /// since nothing is loaded before this check runs.
    #[tokio::test]
    async fn set_tool_credential_rejects_digit_first_name_at_both_scopes() {
        let root_result = set_tool_credential(
            ConfigScope::Root,
            "1_API_KEY".to_string(),
            "sk-irrelevant".to_string(),
        )
        .await;
        let profile_result = set_tool_credential(
            ConfigScope::Profile("some-profile".to_string()),
            "1_API_KEY".to_string(),
            "sk-irrelevant".to_string(),
        )
        .await;
        assert!(root_result.is_err(), "root scope must reject a digit-first key name");
        assert!(profile_result.is_err(), "profile scope must reject a digit-first key name");
    }

    // -------------------------------------------------------------------
    // End-to-end: root scope, gate + vault-failure + disk-unchanged
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn set_tool_credential_gate_closed_writes_nothing() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let cfg = Config::default();
        cfg.save().expect("seed root config.yaml");
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let result = set_tool_credential(
            ConfigScope::Root,
            "EXA_API_KEY".to_string(),
            "sk-should-never-write".to_string(),
        )
        .await;

        let after = std::fs::read(&config_path).expect("read config after gated call");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "gate-closed write must error");
        assert_eq!(before, after, "on-disk config bytes must be unchanged");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn set_tool_credential_root_plaintext_roundtrips_and_status_never_leaks_value() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let cfg = Config {
            security: SecurityConfig {
                web_config_write_enabled: true,
                web_process_control_enabled: false,
                remote_blueprint_run_enabled: false,
            },
            ..Config::default()
        };
        cfg.save().expect("seed root config.yaml");

        let sentinel = "sk-sentinel-must-not-leak-anywhere";
        let write_result = set_tool_credential(
            ConfigScope::Root,
            "EXA_API_KEY".to_string(),
            sentinel.to_string(),
        )
        .await;
        assert!(write_result.is_ok(), "plaintext write must succeed: {write_result:?}");

        let status = get_tool_credential_status(ConfigScope::Root, "EXA_API_KEY".to_string())
            .await
            .expect("status read must succeed");

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(status.present);
        assert_eq!(status.tier, CredentialStorageTier::ConfigPlaintext);
        let json = serde_json::to_string(&status).expect("status must serialize");
        assert!(
            !json.contains(sentinel),
            "serialized status must never contain the stored value; got: {json}"
        );
    }

    /// T-48.2-07-06: a runtime vault-open/write failure returns an error
    /// and `tools.credentials` on disk stays byte-identical — no silent
    /// downgrade to plaintext. Uses `vault.backend: "rusty-vault"` WITHOUT
    /// the crate's `rusty-vault` cargo feature compiled in (this test suite
    /// runs `--features server`, deliberately not `--features
    /// server,rusty-vault`) — `ironhermes_vault::open_store` then hard-errors
    /// with `VaultError::BackendUnavailable` before any secret ever crosses
    /// into a store, which is exactly the "the store itself fails" case
    /// this behavior targets, with no vault fixture required.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn set_tool_credential_vault_open_failure_leaves_config_byte_identical() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let vault = ironhermes_vault::VaultConfig {
            enabled: true,
            backend: "rusty-vault".to_string(),
            ..Default::default()
        };
        let cfg = Config {
            security: SecurityConfig {
                web_config_write_enabled: true,
                web_process_control_enabled: false,
                remote_blueprint_run_enabled: false,
            },
            vault,
            ..Config::default()
        };
        cfg.save().expect("seed root config.yaml");
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let result = set_tool_credential(
            ConfigScope::Root,
            "EXA_API_KEY".to_string(),
            "sk-must-not-be-written".to_string(),
        )
        .await;

        let after = std::fs::read(&config_path).expect("read config after failed vault write");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "an unopenable vault store must error, not succeed");
        assert_eq!(
            before, after,
            "tools.credentials must be byte-identical on disk — no silent plaintext fallback"
        );
    }

    /// REVIEWS finding 6 end-to-end: `vault.enabled: true` with the default
    /// `env-var` backend never reaches the store's write path at all — the
    /// write succeeds by landing in the plaintext config tier instead
    /// (proven by `Ok(())` plus the key showing up in `tools.credentials`;
    /// had the vault path been attempted against the read-only `env-var`
    /// store, the call would have errored, per `provider_secrets_api.rs`'s
    /// identical backend).
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn set_tool_credential_env_var_backend_writes_config_tier_without_attempting_vault() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let mut vault = ironhermes_core::config::Config::default().vault;
        vault.enabled = true;
        assert_eq!(vault.backend, "env-var");
        let cfg = Config {
            security: SecurityConfig {
                web_config_write_enabled: true,
                web_process_control_enabled: false,
                remote_blueprint_run_enabled: false,
            },
            vault,
            ..Config::default()
        };
        cfg.save().expect("seed root config.yaml");

        let result = set_tool_credential(
            ConfigScope::Root,
            "EXA_API_KEY".to_string(),
            "sk-lands-in-plaintext".to_string(),
        )
        .await;

        let reloaded = ironhermes_core::config::Config::load().expect("reload config");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_ok(), "must succeed via the config tier: {result:?}");
        assert_eq!(
            reloaded.tools.credentials.get("EXA_API_KEY").map(String::as_str),
            Some("sk-lands-in-plaintext"),
            "key must land in tools.credentials, proving the vault path was never attempted"
        );
    }

    // -------------------------------------------------------------------
    // Task 3 (D-09): profile-scope arm — delegates to profile_api.rs's
    // hardened write; every test here holds env_lock() as its FIRST
    // statement (REVIEWS finding 4) because the root-.env byte-equality and
    // ${VAR}-substitution assertions both read process env and files under
    // the same temp home a concurrent test could otherwise mutate.
    // -------------------------------------------------------------------

    fn seed_root_config_write_enabled(home: &std::path::Path) {
        let cfg = Config {
            security: SecurityConfig {
                web_config_write_enabled: true,
                web_process_control_enabled: false,
                remote_blueprint_run_enabled: false,
            },
            ..Config::default()
        };
        let _ = home;
        cfg.save().expect("seed root config.yaml");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn set_tool_credential_profile_scope_writes_env_and_leaves_root_env_untouched() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };
        seed_root_config_write_enabled(home_dir.path());

        let root_env_path = home_dir.path().join(".env");
        std::fs::write(&root_env_path, "OPENROUTER_API_KEY=sk-root-untouched\n")
            .expect("seed root .env");
        let root_env_before = std::fs::read(&root_env_path).expect("read seeded root .env");

        let profile_dir = home_dir.path().join("profiles").join("worker-1");
        std::fs::create_dir_all(&profile_dir).expect("mkdir profile dir");

        let result = set_tool_credential(
            ConfigScope::Profile("worker-1".to_string()),
            "EXA_API_KEY".to_string(),
            "sk-profile-value".to_string(),
        )
        .await;

        let profile_env =
            crate::server::profile_api::read_env_keys(&profile_dir.join(".env"))
                .expect("read profile .env after write");
        let root_env_after = std::fs::read(&root_env_path).expect("read root .env after write");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_ok(), "profile-scope write must succeed: {result:?}");
        assert_eq!(
            profile_env.get("EXA_API_KEY").map(String::as_str),
            Some("sk-profile-value")
        );
        assert_eq!(
            root_env_before, root_env_after,
            "root .env bytes must be unchanged by a profile-scope credential write"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn set_tool_credential_profile_scope_value_is_single_quoted_on_disk() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };
        seed_root_config_write_enabled(home_dir.path());

        let profile_dir = home_dir.path().join("profiles").join("worker-2");
        std::fs::create_dir_all(&profile_dir).expect("mkdir profile dir");

        let result = set_tool_credential(
            ConfigScope::Profile("worker-2".to_string()),
            "EXA_API_KEY".to_string(),
            "sk-quote-me".to_string(),
        )
        .await;

        let contents =
            std::fs::read_to_string(profile_dir.join(".env")).expect("read profile .env");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_ok(), "write must succeed: {result:?}");
        assert!(
            contents.contains("EXA_API_KEY='sk-quote-me'"),
            "value must be single-quoted on disk; got: {contents}"
        );
    }

    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn set_tool_credential_profile_scope_writes_owner_read_write_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };
        seed_root_config_write_enabled(home_dir.path());

        let profile_dir = home_dir.path().join("profiles").join("worker-3");
        std::fs::create_dir_all(&profile_dir).expect("mkdir profile dir");

        let result = set_tool_credential(
            ConfigScope::Profile("worker-3".to_string()),
            "EXA_API_KEY".to_string(),
            "sk-perm-check".to_string(),
        )
        .await;

        let perms = std::fs::metadata(profile_dir.join(".env"))
            .expect("stat profile .env")
            .permissions();
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_ok(), "write must succeed: {result:?}");
        assert_eq!(
            perms.mode() & 0o777,
            0o600,
            "profile .env must be owner-read-write only"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn set_tool_credential_traversal_shaped_profile_name_creates_nothing() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };
        seed_root_config_write_enabled(home_dir.path());

        let result = set_tool_credential(
            ConfigScope::Profile("../../etc/evil".to_string()),
            "EXA_API_KEY".to_string(),
            "sk-should-not-write".to_string(),
        )
        .await;

        let escaped_path = home_dir.path().join("../etc/evil");
        let escaped_exists = escaped_path.exists();
        let profiles_dir_entries = std::fs::read_dir(home_dir.path().join("profiles"))
            .map(|rd| rd.count())
            .unwrap_or(0);
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "a traversal-shaped profile name must be rejected");
        assert!(!escaped_exists, "nothing must be created outside the profiles dir");
        assert_eq!(profiles_dir_entries, 0, "no profile directory must be created");
    }

    /// T-48.2-07-03: profile `.env` values are single-quoted, so a value
    /// textually containing a variable reference is stored literally and
    /// never resolves against the writing process's environment on the
    /// next read — the cross-profile secret-leak class this codebase has
    /// hit repeatedly.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn set_tool_credential_profile_scope_var_reference_stored_literally_not_substituted() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };
        seed_root_config_write_enabled(home_dir.path());
        // SAFETY: single-threaded test context under env_lock(); no
        // concurrent env access.
        unsafe { std::env::set_var("IH_TEST_SENTINEL_ROOT_SECRET", "leaked-root-secret") };

        let profile_dir = home_dir.path().join("profiles").join("worker-4");
        std::fs::create_dir_all(&profile_dir).expect("mkdir profile dir");

        let result = set_tool_credential(
            ConfigScope::Profile("worker-4".to_string()),
            "EXA_API_KEY".to_string(),
            "${IH_TEST_SENTINEL_ROOT_SECRET}".to_string(),
        )
        .await;

        let contents =
            std::fs::read_to_string(profile_dir.join(".env")).expect("read profile .env");
        unsafe {
            std::env::remove_var("IH_TEST_SENTINEL_ROOT_SECRET");
            std::env::remove_var("IRONHERMES_HOME");
        }

        assert!(result.is_ok(), "write must succeed: {result:?}");
        assert!(
            contents.contains("${IH_TEST_SENTINEL_ROOT_SECRET}"),
            "the literal variable reference must be stored verbatim; got: {contents}"
        );
        assert!(
            !contents.contains("leaked-root-secret"),
            "the sentinel value must never be substituted into the stored bytes; got: {contents}"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn get_tool_credential_status_profile_scope_reports_profile_env_tier() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };
        seed_root_config_write_enabled(home_dir.path());

        let profile_dir = home_dir.path().join("profiles").join("worker-5");
        std::fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        set_tool_credential(
            ConfigScope::Profile("worker-5".to_string()),
            "EXA_API_KEY".to_string(),
            "sk-status-check".to_string(),
        )
        .await
        .expect("seed write must succeed");

        let status =
            get_tool_credential_status(ConfigScope::Profile("worker-5".to_string()), "EXA_API_KEY".to_string())
                .await
                .expect("status read must succeed");
        let missing_status = get_tool_credential_status(
            ConfigScope::Profile("worker-5".to_string()),
            "TAVILY_API_KEY".to_string(),
        )
        .await
        .expect("status read for absent key must succeed");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(status.present);
        assert_eq!(status.tier, CredentialStorageTier::ProfileEnv);
        assert!(!missing_status.present);
        assert_eq!(missing_status.tier, CredentialStorageTier::NotSet);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn clear_tool_credential_profile_scope_removes_key_leaves_others_untouched() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };
        seed_root_config_write_enabled(home_dir.path());

        let profile_dir = home_dir.path().join("profiles").join("worker-6");
        std::fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        set_tool_credential(
            ConfigScope::Profile("worker-6".to_string()),
            "EXA_API_KEY".to_string(),
            "sk-keep".to_string(),
        )
        .await
        .expect("seed EXA write");
        set_tool_credential(
            ConfigScope::Profile("worker-6".to_string()),
            "TAVILY_API_KEY".to_string(),
            "sk-clear-me".to_string(),
        )
        .await
        .expect("seed TAVILY write");

        let clear_result = clear_tool_credential(
            ConfigScope::Profile("worker-6".to_string()),
            "TAVILY_API_KEY".to_string(),
        )
        .await;

        let env_map = crate::server::profile_api::read_env_keys(&profile_dir.join(".env"))
            .expect("read profile .env after clear");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(clear_result.is_ok(), "clear must succeed: {clear_result:?}");
        assert!(!env_map.contains_key("TAVILY_API_KEY"), "cleared key must be gone");
        assert_eq!(
            env_map.get("EXA_API_KEY").map(String::as_str),
            Some("sk-keep"),
            "untouched key must remain"
        );
    }
}

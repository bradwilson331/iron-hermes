//! `ToolCredentials` — the D-18/D-19 env → config → vault credential snapshot
//! (Phase 41.3 Plan 10).
//!
//! # Why a snapshot exists (D-19)
//!
//! `SecretStore::get_secret` is `async` (`ironhermes-vault/src/lib.rs:48`), but
//! `Tool::is_available(&self)` is sync and takes no `Config`/store handle
//! (`registry.rs`). The vault can therefore never be consulted from inside
//! `is_available()`. Instead, [`ToolCredentials::resolve`] walks the vault **once**,
//! ahead of time, at the async tool-registration seam, and every tool then answers
//! synchronously by reading this struct.
//!
//! # Precedence (D-19)
//!
//! env → config → vault, mirroring `ProviderResolver::apply_vault_fallback`'s D-07
//! order exactly (`provider.rs:575-586`): an earlier tier's hit ends resolution for
//! that key, and the vault is never even consulted for a key an earlier tier already
//! satisfied.
//!
//! # Secret hygiene (T-41.3-49, T-41.3-50, T-41.3-51)
//!
//! Every value that crosses this boundary is held as `secrecy::SecretString`.
//! `Debug` is hand-written (below) to print key **names** and **counts** only —
//! never a value. `Display` and `Serialize` are deliberately not implemented, and
//! the secret-exposing conversion happens at exactly one call site in this module
//! (see `config_field_present_in`'s doc comment) — the same single-boundary
//! discipline `apply_vault_fallback` uses (`provider.rs:565-568`).

use std::collections::BTreeMap;

use secrecy::SecretString;

/// Phase 41.3 D-19: canonical env-var names the vault tier resolves for. These are
/// the SAME names `EnvVarStore::new()`'s `std::env::var(k)` lookup and the
/// `tools.credentials` config map are keyed by — D-19 requires one key name to work
/// across all three tiers (`env_var_store.rs:4-8`'s explicit refusal to invent a
/// project-prefixed key-naming scheme of its own).
pub const TOOL_CREDENTIAL_KEYS: &[&str] = &[
    "FIRECRAWL_API_KEY",
    "EXA_API_KEY",
    "TAVILY_API_KEY",
    "BRAVE_API_KEY",
    "PERPLEXITY_API_KEY",
];

/// D-19: which tier satisfied a credential key. Surfaced for `doctor` (Plan 09) —
/// never for logging a value, since the variant carries no secret material.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    Env,
    Config,
    Vault,
}

impl std::fmt::Debug for CredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CredentialSource::Env => "Env",
            CredentialSource::Config => "Config",
            CredentialSource::Vault => "Vault",
        })
    }
}

/// D-18/D-19: a once-resolved, synchronously-queryable snapshot of tool credentials.
///
/// Holds exactly two pre-resolved maps and nothing else — see the module doc for
/// why a snapshot exists and what precedence it enforces.
pub struct ToolCredentials {
    /// The `Config` passed to `resolve()`, flattened to dotted-path keys ->
    /// `SecretString`. Every config scalar is treated as sensitive rather than
    /// classified (T-41.3-51) — `config.yaml` demonstrably holds API keys, and a
    /// classifier that guesses wrong leaks; over-wrapping only costs a little
    /// memory. Built once via `serde_yaml::to_value(config)` — the same
    /// struct-to-`Value` transform `Config::save()` uses (`config.rs:3491`) — so
    /// this snapshot reflects exactly the in-memory `Config` `resolve()` was given,
    /// not a possibly-stale on-disk file. This is a one-time bulk flatten, not a
    /// second implementation of `config_setter`'s point-lookup `get_at`/`set_at` —
    /// the wizard and `doctor` continue to read/write `config.yaml` through that
    /// reader unchanged. Serves both `config_field_present()` (D-18, arbitrary
    /// dotted paths) and the `tools.credentials.<KEY>` middle tier below.
    config_scalars: BTreeMap<String, SecretString>,
    /// Canonical env-var name -> `SecretString`. Populated only for
    /// `TOOL_CREDENTIAL_KEYS` entries that neither the process env nor
    /// `tools.credentials` satisfied at `resolve()` time (D-19 precedence).
    vault_hits: BTreeMap<String, SecretString>,
}

impl ToolCredentials {
    /// D-19: a snapshot with empty config/vault tiers. `credential()` and
    /// `has_credential()` then answer identically to a bare `std::env::var(..)`
    /// check, because env lookups are always live (see `credential()`'s doc
    /// comment) — this is what every existing call site that never receives a real
    /// snapshot gets, so no existing behavior changes. Also the `Default` impl.
    pub fn env_only() -> Self {
        Self {
            config_scalars: BTreeMap::new(),
            vault_hits: BTreeMap::new(),
        }
    }

    /// D-19: resolve the env → config → vault snapshot once. `store` is `Some(..)`
    /// only when the operator enabled the vault (`vault.enabled: true`) — an
    /// operator who never turned the vault on can never be stopped from booting by
    /// it (T-41.3-53; see `disabled_vault_is_never_opened`).
    ///
    /// Walks `TOOL_CREDENTIAL_KEYS` and, for each key: if the process env has it,
    /// records nothing and moves on **without touching the store**; else if
    /// `tools.credentials` has a non-empty value for it, likewise; else, when a
    /// store was supplied, calls `store.get_secret(key)`.
    ///
    /// # Errors
    ///
    /// Propagates a sealed/corrupt vault's `Err` loudly (D-19 hard error) via `?` —
    /// mirroring `apply_vault_fallback`'s D-07 posture exactly. Never softened to a
    /// `warn` or a partially-populated snapshot.
    pub async fn resolve(
        config: &ironhermes_core::Config,
        store: Option<&dyn ironhermes_vault::SecretStore>,
    ) -> anyhow::Result<Self> {
        let config_scalars = flatten_config_scalars(config)?;

        let mut vault_hits = BTreeMap::new();
        for key in TOOL_CREDENTIAL_KEYS {
            // Priority 1: env. Never even check config or the vault for this key.
            if std::env::var(key).is_ok() {
                continue;
            }
            // Priority 2: tools.credentials.<KEY>. Never consult the vault for this
            // key once config has satisfied it.
            if config_field_present_in(&config_scalars, &format!("tools.credentials.{key}")) {
                continue;
            }
            // Priority 3: vault, only if the operator enabled it. `?` propagates a
            // sealed/corrupt vault's Err loudly (D-19 hard error); `Ok(None)` leaves
            // the key unresolved (healthy vault, key absent — not an error).
            if let Some(store) = store
                && let Some(secret) = store.get_secret(key).await?
            {
                vault_hits.insert((*key).to_string(), secret);
            }
        }

        Ok(Self {
            config_scalars,
            vault_hits,
        })
    }

    /// D-19: the resolved credential for `env_name`, or `None`. Env lookups are
    /// **live** — `std::env::var` is read at query time, not from a pre-resolved
    /// map — falling through to the pre-resolved config then vault tiers only when
    /// the env var is absent. This is not a weakening of "resolve once": the async
    /// vault source is what had to be pre-resolved; env was always synchronous, so
    /// every existing call site that reads the env directly gets the same answer
    /// through this accessor, and `env_only()`/`Default` snapshots behave exactly
    /// like a bare env lookup.
    pub fn credential(&self, env_name: &str) -> Option<SecretString> {
        if let Ok(v) = std::env::var(env_name) {
            return Some(SecretString::from(v));
        }
        let config_path = format!("tools.credentials.{env_name}");
        if let Some(s) = self.config_scalars.get(&config_path) {
            return Some(s.clone());
        }
        self.vault_hits.get(env_name).cloned()
    }

    /// Convenience presence check over `credential()`.
    pub fn has_credential(&self, env_name: &str) -> bool {
        self.credential(env_name).is_some()
    }

    /// D-19: which tier answered `credential(env_name)`, for `doctor` (Plan 09).
    /// Mirrors `credential()`'s precedence exactly (env is live; config and vault
    /// read the pre-resolved maps).
    pub fn source_of(&self, env_name: &str) -> Option<CredentialSource> {
        if std::env::var(env_name).is_ok() {
            return Some(CredentialSource::Env);
        }
        let config_path = format!("tools.credentials.{env_name}");
        if self.config_scalars.contains_key(&config_path) {
            return Some(CredentialSource::Config);
        }
        if self.vault_hits.contains_key(env_name) {
            return Some(CredentialSource::Vault);
        }
        None
    }

    /// D-18: does `dotted_path` resolve to a non-empty scalar in the snapshot's
    /// flattened config? **This is the semantics the `config_field` prerequisite
    /// arm's gating inherits** (Task 2, `registry.rs`): present means the path
    /// resolves to a scalar whose string form is non-empty; absent covers both a
    /// missing path and an explicitly-empty-string value.
    pub fn config_field_present(&self, dotted_path: &str) -> bool {
        config_field_present_in(&self.config_scalars, dotted_path)
    }
}

impl Default for ToolCredentials {
    fn default() -> Self {
        Self::env_only()
    }
}

/// Hand-written `Debug`: key **names** and **counts** only, never a value
/// (T-41.3-49). Do not derive `Debug` on `ToolCredentials` — a derived impl would
/// enumerate `BTreeMap` entries in a way that invites a future field addition to
/// leak a value by accident. This impl is the single place that decides what is
/// safe to print. `vault_hit_keys` is safe: canonical env-var **names**
/// (`TOOL_CREDENTIAL_KEYS` members), never the resolved values.
impl std::fmt::Debug for ToolCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolCredentials")
            .field("config_scalar_count", &self.config_scalars.len())
            .field("vault_hit_count", &self.vault_hits.len())
            .field(
                "vault_hit_keys",
                &self.vault_hits.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// The single secret-exposing call site in this module (see the module doc) —
/// needed only to check whether a resolved scalar's string form is non-empty,
/// matching `config_field_present()`'s documented semantics. The exposed value is
/// compared to emptiness and immediately discarded; it is never returned, logged,
/// or formatted.
fn config_field_present_in(scalars: &BTreeMap<String, SecretString>, dotted_path: &str) -> bool {
    use secrecy::ExposeSecret;
    scalars
        .get(dotted_path)
        .map(|s| !s.expose_secret().is_empty())
        .unwrap_or(false)
}

/// D-18/D-19: flatten `config` (via the same `serde_yaml::to_value` transform
/// `Config::save()` uses, `config.rs:3491`) to dotted-path keys -> `SecretString`.
fn flatten_config_scalars(
    config: &ironhermes_core::Config,
) -> anyhow::Result<BTreeMap<String, SecretString>> {
    let doc = serde_yaml::to_value(config)?;
    let mut out = BTreeMap::new();
    flatten_value(&doc, String::new(), &mut out);
    Ok(out)
}

/// Recursive scalar-leaf flatten over a `serde_yaml::Value` tree. Every
/// string/bool/number leaf is wrapped as a `SecretString` and inserted under its
/// dotted path; sequences, null, and tagged values are skipped — none of them has a
/// single dotted-path scalar leaf to gate on, and no known credential shape needs
/// list indexing.
fn flatten_value(
    value: &serde_yaml::Value,
    prefix: String,
    out: &mut BTreeMap<String, SecretString>,
) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                let Some(key) = k.as_str() else { continue };
                let path = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_value(v, path, out);
            }
        }
        serde_yaml::Value::String(s) => {
            out.insert(prefix, SecretString::from(s.clone()));
        }
        serde_yaml::Value::Bool(b) => {
            out.insert(prefix, SecretString::from(b.to_string()));
        }
        serde_yaml::Value::Number(n) => {
            out.insert(prefix, SecretString::from(n.to_string()));
        }
        _ => {}
    }
}

//! Host facade for Phase 51's per-profile credential endpoint (D-11/D-12/D-13/D-14).
//!
//! `ironhermes-kanban`'s `DispatcherContext` needs a handle to the running endpoint so the
//! gateway-hosted dispatch loop AND both CLI one-shot dispatch paths all get it for free — but
//! `ironhermes-kanban` depends on neither `ironhermes-vault` nor `ironhermes-trajectory`
//! (confirmed in `crates/ironhermes-kanban/Cargo.toml`: only `ironhermes-core`, `-tools`,
//! `-artifacts`). This module is the thin facade that lets it hold that handle anyway: the
//! [`ProfileCredentialHost`] type and [`host_profile_credentials`] function are ALWAYS defined
//! (regardless of the `rusty-vault` cargo feature), so `DispatcherContext`'s field type is
//! stable across feature configurations — only their INTERNALS differ by `#[cfg]`.
//!
//! # WIDENED PROTOCOL — see `crates/ironhermes-vault/src/profile_endpoint.rs`'s module doc
//!
//! `51-CONTEXT.md`'s D-11/D-12/D-13 originally locked a request shape with no field capable of
//! naming a profile. At this plan's Task 1 checkpoint the user explicitly selected the `widen`
//! option instead, reversing that specific "no profile field" clause — the request may now name
//! a profile, checked against (never trusted over) the presented token's own derived slug. This
//! facade forwards to the widened endpoint unchanged; see `profile_endpoint.rs` and
//! `51-06-SUMMARY.md` for the full record.

use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;

#[cfg(feature = "rusty-vault")]
pub use ironhermes_vault::profile_endpoint::ProfileCredentialEndpointHandle as ProfileCredentialHost;

/// Feature-off stand-in for [`ProfileCredentialHost`] — deliberately uninhabited
/// (`std::convert::Infallible`) so it can never actually be constructed; `host_profile_credentials`
/// on this feature arm always returns `None`, matching D-14 (feature not compiled in -> no
/// endpoint, ever). Exists purely so `DispatcherContext`'s `Option<Arc<ProfileCredentialHost>>`
/// field type compiles identically whether or not the `rusty-vault` feature is active.
#[cfg(not(feature = "rusty-vault"))]
#[derive(Debug)]
pub struct ProfileCredentialHost {
    _never: std::convert::Infallible,
}

#[cfg(not(feature = "rusty-vault"))]
impl ProfileCredentialHost {
    /// Never reachable — `self._never` is uninhabited, so this function can never actually be
    /// called on a real instance. Present only so callers written against the feature-on API
    /// (which does have a real `socket_path()`) compile identically either way.
    pub fn socket_path(&self) -> &std::path::Path {
        match self._never {}
    }
}

/// Which lifetime contract a credential-endpoint host declares (Phase 51 Plan 19, G-51-6).
///
/// The original design had exactly one kind of host and never wrote that assumption down:
/// [`host_profile_credentials`] hosted unconditionally, and there was no place in the API where
/// a caller could state, or a callee could ask, "will this process still be alive when my
/// worker connects?". The two long-lived hosts —
/// `crates/ironhermes-gateway/src/runner.rs`'s embedded dispatcher and `cmd_daemon`
/// (`crates/ironhermes-cli/src/kanban/commands.rs`) — satisfied that assumption silently; the
/// one-shot `cmd_dispatch` violated it silently, unlinking its PID-keyed socket the instant it
/// returns while the detached workers it just spawned are still starting. This enum turns the
/// unwritten assumption into a written one at every call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialHostLifetime {
    /// The hosting process stays alive at least until every worker it spawns has completed its
    /// credential bootstrap read. Declared by both long-lived hosts (the gateway runner and
    /// `cmd_daemon`) — the assumption every existing caller was already relying on.
    OutlivesWorkers,
    /// The process returns as soon as it has spawned its detached workers, so the PID-keyed
    /// socket its `ProfileCredentialEndpointHandle` owns is unlinked before those workers can
    /// connect. Declared by `cmd_dispatch` via `DispatcherContext::new_one_shot` — a host with
    /// this lifetime never binds a socket at all (see [`host_profile_credentials_with_lifetime`]).
    ExitsBeforeWorkers,
}

/// Shared by both `#[cfg]` arms of [`host_profile_credentials_with_lifetime`]'s
/// `ExitsBeforeWorkers` branch: never binds anything, but names the reason when the long-lived
/// variant WOULD have hosted, so an operator can see why nothing was bound instead of silence.
/// Feature-independent because the decision to decline is itself feature-independent — a
/// feature-off build declines for a different underlying reason, but the operator-facing
/// message is the same either way.
fn warn_declined_short_lived_host_if_would_have_hosted(config: &Config) {
    if config.vault.enabled && config.vault.backend == "rusty-vault" {
        tracing::warn!(
            event = "profile_credential_host_declined_short_lived",
            "a short-lived host cannot host the per-profile vault credential endpoint — it \
             would unlink its PID-keyed socket before a detached worker could connect; \
             vault-backed dispatch needs a long-lived dispatcher (`ironhermes kanban daemon \
             --force`, or the gateway-embedded dispatcher)"
        );
    }
}

/// The general hosting entry point (Phase 51 Plan 19, G-51-6 — promoted from
/// [`host_profile_credentials`], which is now a one-line wrapper naming the variant its
/// existing callers were always relying on). Returns `None` — never panics, never partially
/// hosts — whenever the vault is disabled, not configured for the `rusty-vault` backend, the
/// feature is not compiled in, the vault could not be opened/is sealed, or `lifetime` is
/// [`CredentialHostLifetime::ExitsBeforeWorkers`] (which never binds anything, by design: see
/// that variant's own doc). Plan 05's dispatch gate independently refuses vault-backed profiles
/// when this is `None` and no scrubbed `.env` fallback exists (D-14: fail-closed by omission,
/// never a panic and never a partially-hosted listener).
///
/// Requires an active Tokio runtime on the calling thread for the `OutlivesWorkers` branch —
/// internally hosts via
/// [`ironhermes_vault::profile_endpoint::host_profile_credential_endpoint`], which spawns the
/// accept loop via `tokio::spawn`. Every production call site runs inside an async context
/// already; on the default (`vault.enabled == false`) config this function returns `None`
/// before ever reaching that `tokio::spawn` call, so it is safe to call from any context (sync
/// or async, real runtime or none) as long as vault is disabled — which is every install's
/// default. The `ExitsBeforeWorkers` branch never touches the vault, the filesystem, or a
/// socket, so it needs no runtime at all.
#[cfg(feature = "rusty-vault")]
pub fn host_profile_credentials_with_lifetime(
    config: &Config,
    lifetime: CredentialHostLifetime,
) -> Option<Arc<ProfileCredentialHost>> {
    match lifetime {
        CredentialHostLifetime::ExitsBeforeWorkers => {
            warn_declined_short_lived_host_if_would_have_hosted(config);
            None
        }
        CredentialHostLifetime::OutlivesWorkers => {
            if !(config.vault.enabled && config.vault.backend == "rusty-vault") {
                return None;
            }
            let vault_cfg = crate::vault::resolve_vault_config(config);
            let sink: Arc<dyn ironhermes_vault::ProfileGuardAudit> =
                Arc::new(ironhermes_vault::TracingProfileGuardAudit);
            ironhermes_vault::profile_endpoint::host_profile_credential_endpoint(
                &vault_cfg.rusty_vault,
                sink,
            )
            .map(Arc::new)
        }
    }
}

/// Feature-off arm: `ExitsBeforeWorkers` still declines-and-names (feature-independent — see
/// [`warn_declined_short_lived_host_if_would_have_hosted`]); `OutlivesWorkers` is always `None`
/// and touches neither `config` nor the filesystem (D-14 — "the feature is not compiled in" is
/// one of the three yields-no-endpoint conditions).
#[cfg(not(feature = "rusty-vault"))]
pub fn host_profile_credentials_with_lifetime(
    config: &Config,
    lifetime: CredentialHostLifetime,
) -> Option<Arc<ProfileCredentialHost>> {
    if lifetime == CredentialHostLifetime::ExitsBeforeWorkers {
        warn_declined_short_lived_host_if_would_have_hosted(config);
    }
    None
}

/// The single facade `DispatcherContext` construction calls for a host that outlives its
/// workers (Phase 51 D-11/D-14). A one-line delegation to
/// [`host_profile_credentials_with_lifetime`] with [`CredentialHostLifetime::OutlivesWorkers`]
/// — the variant every existing caller of this function was already relying on without stating
/// it (Phase 51 Plan 19, G-51-6). Signature unchanged: `DispatcherContext::new` and
/// `with_spawn_fn` keep calling this exact function so
/// `both_constructors_wire_profile_credentials_through_the_same_function` still counts exactly
/// two occurrences of its wiring needle.
#[cfg(feature = "rusty-vault")]
pub fn host_profile_credentials(config: &Config) -> Option<Arc<ProfileCredentialHost>> {
    host_profile_credentials_with_lifetime(config, CredentialHostLifetime::OutlivesWorkers)
}

/// Feature-off arm: same one-line delegation as the feature-on arm above — always `None`.
#[cfg(not(feature = "rusty-vault"))]
pub fn host_profile_credentials(config: &Config) -> Option<Arc<ProfileCredentialHost>> {
    host_profile_credentials_with_lifetime(config, CredentialHostLifetime::OutlivesWorkers)
}

#[cfg(test)]
mod host_lifetime_tests {
    use super::*;

    /// Phase 51 Plan 19 (G-51-6) Task 1: a short-lived host declines by construction — no
    /// endpoint returned, and no PID-keyed socket file left behind — even when the config is
    /// genuinely vault-enabled with the `rusty-vault` backend (the exact config that WOULD have
    /// caused `OutlivesWorkers` to bind). Runs regardless of the `rusty-vault` feature: the
    /// `ExitsBeforeWorkers` branch is feature-independent by design.
    #[test]
    fn declined_short_lived_host_never_binds_a_socket() {
        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.backend = "rusty-vault".to_string();

        let result = host_profile_credentials_with_lifetime(
            &config,
            CredentialHostLifetime::ExitsBeforeWorkers,
        );
        assert!(
            result.is_none(),
            "a short-lived host must never bind an endpoint"
        );

        let expected_socket =
            std::env::temp_dir().join(format!("ihvc-{}.sock", std::process::id()));
        assert!(
            !expected_socket.exists(),
            "a short-lived host must not leave a PID-keyed socket file behind: {}",
            expected_socket.display()
        );
    }
}

// =============================================================================
// Phase 51 Plan 07 — mint-at-dispatch (D-07/D-14)
// =============================================================================
//
// `ironhermes-kanban`'s `DispatcherContext` needs to mint a bootstrap-only token for a
// `DispatchDecision::AllowFromVault` dispatch, and to ledger the mint's accessor —
// but, exactly like `host_profile_credentials` above, `ironhermes-kanban` depends on
// neither `ironhermes-vault` (the mint implementation) nor `ironhermes-trajectory`
// (the ledger). This facade is the crossing point for both: [`ProfileTokenAudit`] and
// [`mint_worker_credential`] are ALWAYS defined (regardless of the `rusty-vault`
// feature), and [`TrajectoryProfileTokenAudit`] adapts Plan 03's audit-sink contract
// onto `ironhermes-core`'s own `TrajectoryWriterHandle` trait so the three production
// `DispatcherContext` construction sites (which already depend on
// `ironhermes-trajectory`) can supply a real, ledgered sink through this facade alone.

/// Always-defined audit-sink trait (Phase 51 Plan 07, D-07) — mirrors
/// [`ProfileCredentialHost`]'s feature-stable-type pattern above. Feature-on: the real
/// `ironhermes_vault::ProfileTokenAudit` (identical shape, re-exported). Feature-off:
/// an identical-shaped stand-in so `DispatcherContext::token_audit`'s field type
/// compiles either way. A mint call records the profile slug, the token's ACCESSOR
/// (never the token itself), and the granted TTL — never called for a failed mint.
#[cfg(feature = "rusty-vault")]
pub use ironhermes_vault::ProfileTokenAudit;

#[cfg(not(feature = "rusty-vault"))]
pub trait ProfileTokenAudit: Send + Sync {
    /// Record a successful mint: the profile slug, the token's ACCESSOR (never the
    /// token itself), and the granted TTL.
    fn record_mint(&self, slug: &str, accessor: &str, ttl: Duration) -> anyhow::Result<()>;
}

/// Adapts Plan 03's [`ProfileTokenAudit`] onto core's own
/// [`crate::commands::context::TrajectoryWriterHandle`] trait, so
/// `ironhermes-kanban`'s dispatcher — which depends on neither `ironhermes-vault` nor
/// `ironhermes-trajectory` — can supply a real, ledgered audit sink via this facade
/// alone. Writes one JSONL line per successful mint: `slug`, `accessor`, `ttl_secs`,
/// and a timestamp — never the token's secret bytes.
pub struct TrajectoryProfileTokenAudit {
    writer: Arc<dyn crate::commands::context::TrajectoryWriterHandle>,
}

impl TrajectoryProfileTokenAudit {
    /// Wrap an existing trajectory-writer handle (the gateway runner and both CLI
    /// dispatch call sites already construct one for their own command context).
    pub fn new(writer: Arc<dyn crate::commands::context::TrajectoryWriterHandle>) -> Self {
        Self { writer }
    }
}

impl ProfileTokenAudit for TrajectoryProfileTokenAudit {
    fn record_mint(&self, slug: &str, accessor: &str, ttl: Duration) -> anyhow::Result<()> {
        let line = serde_json::json!({
            "event": "profile_token_minted",
            "slug": slug,
            "accessor": accessor,
            "ttl_secs": ttl.as_secs(),
            "ts": chrono::Utc::now().to_rfc3339(),
        })
        .to_string();
        self.writer.append_json_line(&line)
    }
}

/// The mint result a vault-backed dispatch needs to pass into the worker's spawn
/// environment (Phase 51 Plan 07). Deliberately plain, owned types
/// (`secrecy::SecretString`/`String`/`PathBuf`) rather than
/// `ironhermes_vault::MintedProfileToken` — `ironhermes-kanban` cannot depend on
/// `ironhermes-vault` (see this module's doc), so this is the crossing point that
/// strips the vault-specific type down to what a kanban-local `WorkerVaultBootstrap`
/// needs to hold.
#[derive(Debug)]
pub struct MintedWorkerCredential {
    /// The minted, short-lived, single-policy token — never the raw provider key.
    pub token: secrecy::SecretString,
    /// The opaque accessor recorded to the audit sink — safe to log.
    pub accessor: String,
    /// The socket path the worker connects to for its single bootstrap read.
    pub socket_path: std::path::PathBuf,
}

/// Named refusal reasons a vault-backed mint attempt can fail with (Phase 51 Plan 07,
/// D-07/D-14). A mint failure of any kind means the dispatcher must refuse the spawn —
/// there is no partial path where a worker is created and then discovers it has no
/// credential.
#[derive(Debug, thiserror::Error)]
pub enum MintCredentialError {
    /// The vault is disabled, not configured for the `rusty-vault` backend, or the
    /// feature is not compiled in — mint can never be reachable.
    #[error(
        "vault disabled, not configured for the rusty-vault backend, or the rusty-vault feature is not compiled in"
    )]
    VaultUnavailable,
    /// The mint attempt itself failed (vault unreachable, denied, or an opaque
    /// backend error) — wraps the underlying error's message.
    #[error("mint failed: {0}")]
    Mint(String),
}

/// Mint a bootstrap-only token for `slug` and pair it with the running endpoint's
/// socket path (Phase 51 Plan 07, D-07). Requires the SAME `Config` `host` was built
/// from, and the already-hosted `host` itself so the returned socket path matches the
/// endpoint the worker will actually connect to. Ledgering happens INSIDE this call —
/// `sink.record_mint` is invoked by the underlying `mint_profile_token` only after the
/// vault mint has genuinely succeeded, so "mint, then ledger, then spawn" is satisfied
/// by this function's own internal ordering; the caller does not need a separate
/// ledger step.
#[cfg(feature = "rusty-vault")]
pub async fn mint_worker_credential(
    config: &Config,
    host: &ProfileCredentialHost,
    slug: &str,
    sink: &dyn ProfileTokenAudit,
) -> Result<MintedWorkerCredential, MintCredentialError> {
    if !(config.vault.enabled && config.vault.backend == "rusty-vault") {
        return Err(MintCredentialError::VaultUnavailable);
    }
    // Phase 51 Plan 11 (CR-06 fix): mint through the passed host's own `Core`, not a second,
    // independent one — see `ironhermes_vault::mint_profile_token_for_host`'s doc. This opens
    // no `Core` of its own.
    let minted = ironhermes_vault::mint_profile_token_for_host(
        host,
        slug,
        ironhermes_vault::profile_token_ttl_for_bootstrap(),
        sink,
    )
    .await
    .map_err(|e| MintCredentialError::Mint(e.to_string()))?;
    Ok(MintedWorkerCredential {
        token: secrecy::SecretString::from(
            secrecy::ExposeSecret::expose_secret(minted.token()).to_string(),
        ),
        accessor: minted.accessor().to_string(),
        socket_path: host.socket_path().to_path_buf(),
    })
}

/// Feature-off arm: mint can never be reachable — the vault branch of the dispatch
/// gate that produces `AllowFromVault` is itself feature-gated to the identical
/// condition, so this arm should never actually be called in practice; it exists so
/// the facade's signature is stable regardless of feature.
#[cfg(not(feature = "rusty-vault"))]
pub async fn mint_worker_credential(
    _config: &Config,
    _host: &ProfileCredentialHost,
    _slug: &str,
    _sink: &dyn ProfileTokenAudit,
) -> Result<MintedWorkerCredential, MintCredentialError> {
    Err(MintCredentialError::VaultUnavailable)
}

// =============================================================================
// Phase 51 Plan 10 — the shared spawn-credential decision (D-07/D-11/D-14/D-15)
// =============================================================================
//
// `ironhermes-kanban`'s dispatcher and `iron_hermes_ui`'s bot spawn path both ask the
// exact same question before creating a child process for a profile: "what credential
// does this child get, if any". `decide_spawn_credential` below is the single, shared
// answer — composed entirely from functions that are ALREADY feature-stable
// (`ironhermes_core::dispatch_gate::evaluate_profile_dispatch_at`, [`mint_worker_credential`]
// above), so this function itself needs no `#[cfg]` of its own, and neither does any
// caller. `ironhermes-kanban` cannot depend on `ironhermes-vault` (see this module's
// top-level doc), which is why this lives here rather than in `ironhermes-vault` directly.
//
// `crates/ironhermes-kanban/src/dispatcher.rs` deliberately keeps its own inline copy of
// this exact gate -> mint -> spawn sequence for now — that path is proven end-to-end by a
// live UAT and unifying it onto this function is explicitly deferred to a later plan
// (Phase 51 Plan 10 scope decision).

/// Which of the two structurally different reasons produced a
/// [`SpawnCredentialDecision::Refuse`] (Phase 51 Plan 14, WR-03). Before this type existed,
/// the only way a caller could tell the two apart was substring-matching the `reason` text
/// against `decide_spawn_credential`'s own `format!` literals — a coupling any reword of
/// those literals (even a cosmetic one) would silently break, reclassifying a mint failure
/// as a gate refusal. The `reason` string itself is UNCHANGED by this type — it is still
/// forwarded verbatim, because D-14's three distinguishable gate refusal reasons, and the
/// three mint-unavailable reasons below, are read directly by operators and existing tests.
/// `source` exists purely so *routing* reads a discriminant instead of prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalSource {
    /// [`ironhermes_core::dispatch_gate::evaluate_profile_dispatch_at`] itself refused —
    /// the profile has no config.yaml, no key, an unknown provider, etc. Nothing was ever
    /// attempted against the vault.
    Gate,
    /// The gate found `AllowFromVault`, but the credential could not actually be produced:
    /// no hosted endpoint, no audit sink, or the mint itself failed.
    MintUnavailable,
}

/// What a child process spawned for a profile should receive, or whether it should be
/// spawned at all (Phase 51 Plan 10). Three variants, mirroring
/// [`ironhermes_core::dispatch_gate::DispatchDecision`]'s own three-variant shape and for
/// the same reason: every future `match` on this becomes non-exhaustive and must be
/// updated consciously.
#[derive(Debug)]
pub enum SpawnCredentialDecision {
    /// The profile's own `.env` resolves a key for its configured provider. No mint, no
    /// vault contact, no ledger line — an un-migrated profile behaves byte-identically to
    /// before this plan.
    Dotenv,
    /// The gate found no `.env` key but the vault holds one, and a scoped, ledgered token
    /// was successfully minted for this profile.
    Vault(MintedWorkerCredential),
    /// No child process should be created. `source` is the typed discriminant a caller
    /// matches on (Phase 51 Plan 14, WR-03); `reason` is still the human-legible text —
    /// either the gate's own reason string (forwarded verbatim, never re-worded), or a
    /// reason naming which prerequisite (hosted endpoint, audit sink, or the mint itself)
    /// was missing.
    Refuse {
        source: RefusalSource,
        reason: String,
    },
}

/// The credential decision every spawn surface makes before creating a child process for
/// `slug` (Phase 51 Plan 10, D-07/D-11/D-14/D-15). Performs exactly the sequence
/// `crates/ironhermes-kanban/src/dispatcher.rs` performs inline today, in the same order
/// and with the same fail-closed posture:
///
/// 1. Call [`ironhermes_core::dispatch_gate::evaluate_profile_dispatch_at`] — the ONLY
///    gate. A refusal becomes the refuse arm, carrying the gate's own reason UNCHANGED —
///    never prepended, re-worded, or wrapped, because D-14's three distinguishable vault
///    refusal reasons already live in that string.
/// 2. `Allow` becomes the dotenv arm. No mint, no vault contact, no ledger line.
/// 3. `AllowFromVault` with either `host` or `sink` absent becomes the refuse arm, naming
///    which one is missing — never a silent downgrade to the dotenv arm, because the
///    profile's `.env` was scrubbed by definition once the gate reaches this branch.
/// 4. `AllowFromVault` with both present calls [`mint_worker_credential`], which ledgers
///    the accessor internally before returning. A mint failure becomes the refuse arm
///    carrying the mint error's message; success becomes the vault arm.
///
/// `config` is the caller's own process `Config` (used only for the mint's vault
/// resolution) — the gate independently re-derives the PROFILE's own config from
/// `profiles_root/slug/config.yaml`, exactly as it always has.
pub async fn decide_spawn_credential(
    config: &Config,
    profiles_root: &std::path::Path,
    slug: &str,
    host: Option<&ProfileCredentialHost>,
    sink: Option<&dyn ProfileTokenAudit>,
) -> SpawnCredentialDecision {
    match crate::dispatch_gate::evaluate_profile_dispatch_at(profiles_root, slug).await {
        crate::dispatch_gate::DispatchDecision::Refuse { reason } => {
            SpawnCredentialDecision::Refuse {
                source: RefusalSource::Gate,
                reason,
            }
        }
        crate::dispatch_gate::DispatchDecision::Allow => SpawnCredentialDecision::Dotenv,
        crate::dispatch_gate::DispatchDecision::AllowFromVault => {
            let host = match host {
                Some(h) => h,
                None => {
                    return SpawnCredentialDecision::Refuse {
                        source: RefusalSource::MintUnavailable,
                        reason: format!(
                            "profile \"{slug}\" resolved AllowFromVault but this process has no \
                             hosted credential endpoint — refusing rather than spawning without a \
                             working credential"
                        ),
                    };
                }
            };
            let sink = match sink {
                Some(s) => s,
                None => {
                    return SpawnCredentialDecision::Refuse {
                        source: RefusalSource::MintUnavailable,
                        reason: format!(
                            "profile \"{slug}\" resolved AllowFromVault but this process has no \
                             audit sink — refusing rather than minting an un-audited credential"
                        ),
                    };
                }
            };
            match mint_worker_credential(config, host, slug, sink).await {
                Ok(minted) => SpawnCredentialDecision::Vault(minted),
                Err(e) => SpawnCredentialDecision::Refuse {
                    source: RefusalSource::MintUnavailable,
                    reason: format!(
                        "profile \"{slug}\" resolved AllowFromVault but the mint failed: {e}"
                    ),
                },
            }
        }
    }
}

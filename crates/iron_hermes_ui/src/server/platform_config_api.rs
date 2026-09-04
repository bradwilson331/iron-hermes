//! Phase 48.2 Plan 12 (D-06/D-08/D-09/D-10/D-11/D-12/D-14, G-48.2-7): the
//! Buzz platform's read/write server surface over `gateway.platforms["buzz"]`
//! (`PlatformGatewayConfig`) — the same gated, `ConfigScope`-parameterized
//! path every write in this phase follows: resolve_scope_target (fresh disk
//! read) -> check_buzz_write_gate -> mutate the EXISTING map entry ->
//! save_scoped -> re-read from disk.
//!
//! # The DTO is an explicit allowlist, never a passthrough (T-48.2-12-01)
//!
//! [`BuzzPlatformView`] names exactly the fields Buzz uses: `enabled`,
//! `whitelist`, `relay_url`, `channels`, and `channel_trust` as a display
//! string. `PlatformGatewayConfig` is SHARED across every platform this
//! gateway hosts and also carries `token`, `app_token`, and `api_key` —
//! Telegram/Discord/Slack bot-token fields with no place in a Buzz DTO. This
//! type exists precisely so a future field added to that shared struct
//! cannot silently start shipping toward the browser: nothing added to
//! `PlatformGatewayConfig` reaches this DTO unless a person deliberately
//! adds a matching field here AND updates
//! [`buzz_platform_view_dto_carries_no_secret_bearing_field`] below.
//!
//! # Model/provider disclosure (Phase 48.2 Plan 14, D-06/D-08/D-10/D-14, G-48.2-8)
//!
//! `BuzzPlatformView.model_disclosure` is a SECOND kind of allowlist entry:
//! it carries derived scalars (provider name, post-overlay model string)
//! copied out of a resolved `ironhermes_core::provider::ResolvedEndpoint` —
//! the endpoint itself, which owns `api_key: Option<String>`
//! (`provider.rs:45`), never enters this DTO. The strict resolver variant
//! used to build it (`build_with_env_overrides_strict`) reduces but does
//! NOT eliminate key presence — a literal `providers.<name>.api_key` in
//! config.yaml can still populate the endpoint. The allowlist copy in
//! [`resolve_buzz_model_disclosure`], enforced by the RECURSIVE
//! serialization check in
//! [`buzz_platform_view_dto_carries_no_secret_bearing_field`], is the
//! actual control — not the resolver constructor.
//!
//! # "Not configured" is its own answer (Task 1)
//!
//! `PlatformGatewayConfig::default()` is `enabled: false` with empty lists —
//! structurally identical to what an operator who explicitly wrote
//! `buzz: {enabled: false}` would see. Rendering an ABSENT `buzz:` block the
//! same way would hide the fact that no such block exists at all, so
//! [`build_buzz_view`] takes `Option<&PlatformGatewayConfig>` and sets
//! `configured: false` only when the key itself is missing from
//! `gateway.platforms` — never derived from `enabled` or any other field.
//!
//! # `BUZZ_NSEC` has no server fn in this module
//!
//! The identity secret's read/write path is entirely 48.2-07's existing
//! `tools_credentials_api`/`buzz_npub_api` machinery — Task 3 mounts the
//! existing `ToolCredentialForm` and `fetch_bot_npub` directly from
//! `buzz_section.rs`. This module never touches `BUZZ_NSEC` at all.
//!
//! # No live-apply (D-12)
//!
//! Unlike `tools_config_api`'s toolset writes, Buzz's config lives inside
//! the SEPARATE gateway process — a write here has no running-process
//! counterpart to notify. `buzz_section.rs` states this once; this module
//! does not call anything resembling `apply_live_toolset_config`.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::server::tools_config_api::ConfigScope;

/// The `gateway.platforms` map key this module reads and writes. Never
/// hardcoded a second time below — every lookup goes through this constant.
const BUZZ_PLATFORM_KEY: &str = "buzz";

// =============================================================================
// DTOs — shared shape on both the wasm client and the native server.
// =============================================================================

/// Explicit field allowlist over `gateway.platforms["buzz"]`
/// (`PlatformGatewayConfig`) — see module doc's "The DTO is an explicit
/// allowlist" section. `configured` distinguishes "no `buzz:` block exists
/// yet" from "the block exists and is disabled" (Task 1) — these are
/// different facts and must never collapse into one.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BuzzPlatformView {
    pub configured: bool,
    pub enabled: bool,
    /// Canonical cross-platform sender allowlist. Empty = deny all — see
    /// `PlatformGatewayConfig::whitelist`'s own doc comment
    /// (`ironhermes-core/src/config.rs`); this DTO carries the same
    /// contract, never a paraphrase of it.
    pub whitelist: Vec<String>,
    pub relay_url: Option<String>,
    pub channels: Vec<String>,
    /// Display string for `ChannelTrust` — `"closed"` or `"open"`. Never
    /// editable through this DTO (Task 2's `channel_trust` prohibition):
    /// read-only by construction, since no write fn in this module ever
    /// accepts a `channel_trust` field.
    pub channel_trust: String,
    /// The resolved provider/model that will serve a Buzz turn for this
    /// scope, plus configured sub-job role overrides (Task 1, G-48.2-8).
    /// See module doc's "Model/provider disclosure" section — this field
    /// carries derived scalars only, never a `ResolvedEndpoint`.
    pub model_disclosure: BuzzModelDisclosure,
}

/// One resolved provider/model pairing — either the reply-serving main
/// resolution or a configured sub-job role override. `label` is `"main"`
/// for the reply-serving resolution or the role name for a role override.
/// `provider` is always a real, operator-recognizable provider name — NEVER
/// the config-vocabulary inheritance token `"main"`, even when the entry it
/// describes inherits the main provider. `model` is the POST-overlay value
/// (`ResolvedEndpoint.default_model` is documented as reflecting the FINAL
/// `default_model`, not a pre-overlay snapshot — `provider.rs:51-56`) —
/// `None` means the named provider is not defined in this config, and the
/// entry is still disclosed rather than dropped.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BuzzModelResolution {
    pub label: String,
    pub provider: String,
    pub model: Option<String>,
}

/// The resolved-runtime disclosure for a scope whose provider configuration
/// resolved successfully. `main` is the resolution that serves a Buzz
/// reply — the ONE resolution the section's headline row shows. `roles`
/// lists every configured `model.roles` entry separately so one label never
/// has to stand in for the whole truth (role overrides apply to specific
/// sub-jobs inside a turn, not to the reply itself). `config_source` names
/// which config file (root vs. a named profile) produced these values
/// (D-08's scope dimension made legible).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BuzzRuntimeDisclosure {
    pub config_source: String,
    pub main: BuzzModelResolution,
    pub roles: Vec<BuzzModelResolution>,
}

/// Whether this scope's provider configuration could be resolved at all.
/// An enum — not two parallel `Option` fields — so the client cannot render
/// a half-state (e.g. a populated `main` alongside a populated `reason`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum BuzzModelDisclosure {
    Resolved(BuzzRuntimeDisclosure),
    Unresolvable { reason: String },
}

// =============================================================================
// Server-only helpers — pure where possible, so tests never need a running
// server or an installed global AppState (mirrors `tools_config_api.rs`'s
// test-reachability discipline, module doc there).
// =============================================================================

/// Display label for `ChannelTrust` — the ONE place this module turns the
/// typed enum into the string the DTO carries.
#[cfg(not(target_arch = "wasm32"))]
fn channel_trust_label(trust: ironhermes_core::config::ChannelTrust) -> String {
    match trust {
        ironhermes_core::config::ChannelTrust::Closed => "closed".to_string(),
        ironhermes_core::config::ChannelTrust::Open => "open".to_string(),
    }
}

/// Pure builder: `entry` is `None` when no `buzz:` block exists in
/// `gateway.platforms` at all — see module doc's "'Not configured' is its
/// own answer" section. `model_disclosure` is computed by the caller (from
/// [`resolve_buzz_model_disclosure`]) and passed in, keeping this fn a pure
/// function with no resolver of its own — the disclosure is independent of
/// whether a `buzz:` block exists, since it answers "what will serve a Buzz
/// turn" from the scope's provider config, not from the platform entry. No
/// disk I/O; directly unit-testable.
#[cfg(not(target_arch = "wasm32"))]
fn build_buzz_view(
    entry: Option<&ironhermes_core::config::PlatformGatewayConfig>,
    model_disclosure: BuzzModelDisclosure,
) -> BuzzPlatformView {
    match entry {
        Some(cfg) => BuzzPlatformView {
            configured: true,
            enabled: cfg.enabled,
            whitelist: cfg.whitelist.clone(),
            relay_url: cfg.relay_url.clone(),
            channels: cfg.channels.clone(),
            channel_trust: channel_trust_label(cfg.channel_trust),
            model_disclosure,
        },
        None => BuzzPlatformView {
            configured: false,
            enabled: false,
            whitelist: Vec::new(),
            relay_url: None,
            channels: Vec::new(),
            channel_trust: channel_trust_label(ironhermes_core::config::ChannelTrust::default()),
            model_disclosure,
        },
    }
}

/// Fixed, input-independent reason returned whenever this scope's provider
/// configuration cannot produce a model resolution — the SAME string
/// regardless of the underlying cause (a resolver build error, or a main
/// provider name the resolver deliberately allows to be unknown at build
/// time so operators can still introspect, diagnosis fact 4). Provider-build
/// errors embed `base_url` verbatim (`provider.rs:446`) and a `base_url` can
/// carry userinfo credentials, so the underlying error is discarded
/// entirely — never summarized, never partially forwarded.
const BUZZ_MODEL_UNRESOLVABLE_REASON: &str = "This scope's provider configuration could not be \
     resolved — check the provider settings in this scope's config.yaml.";

/// Resolve the post-overlay provider/model that will serve a Buzz reply for
/// `scope`, plus every configured `model.roles` sub-job override (Task 1,
/// G-48.2-8). Copies ONLY derived scalars out of the resolver's
/// `ResolvedEndpoint`s — never the endpoint itself (module doc's "Model/
/// provider disclosure" section).
#[cfg(not(target_arch = "wasm32"))]
fn resolve_buzz_model_disclosure(
    config: &ironhermes_core::config::Config,
    scope: &ConfigScope,
) -> BuzzModelDisclosure {
    // STRICT: disables the process-environment fallback so this server
    // process's ambient root .env can never contribute a key into an
    // endpoint this function handles (D-06, same reasoning
    // profile_verify_api.rs's CR-01 note records for its own strict
    // switch). ModelsCache::default() rather than ModelsCache::load():
    // the cache only supplies context-length metadata this DTO does not
    // carry, while load() reads the operator's real home directory and
    // would make this function depend on ambient machine state — the
    // exact hazard build_with_cache's own doc comment documents.
    let resolver =
        match ironhermes_core::provider::ProviderResolver::build_with_env_overrides_strict(
            config,
            ironhermes_core::models_cache::ModelsCache::default(),
            &std::collections::HashMap::new(),
        ) {
            Ok(r) => r,
            Err(_) => {
                return BuzzModelDisclosure::Unresolvable {
                    reason: BUZZ_MODEL_UNRESOLVABLE_REASON.to_string(),
                };
            }
        };

    let main_provider_name = resolver.main_provider().to_string();
    // Per diagnosis fact 4: resolve_for_main() panics on an unknown main
    // provider (provider.rs:642-649), and the resolver is deliberately
    // allowed to build with one anyway (provider.rs:437) so operators can
    // still introspect. Use the non-panicking resolve() so an absent main
    // entry is a reachable operator state that renders honestly instead of
    // aborting the request.
    let main_model = match resolver.resolve(&main_provider_name) {
        Some(endpoint) => endpoint.default_model.clone(),
        None => {
            return BuzzModelDisclosure::Unresolvable {
                reason: BUZZ_MODEL_UNRESOLVABLE_REASON.to_string(),
            };
        }
    };
    let main = BuzzModelResolution {
        label: "main".to_string(),
        provider: main_provider_name.clone(),
        model: Some(main_model),
    };

    // Sub-job role overrides — each disclosed separately (role_override_note
    // requirement) rather than dropped when its provider is undefined.
    let mut roles: Vec<BuzzModelResolution> = config
        .model
        .roles
        .iter()
        .map(|(role_name, role_cfg)| {
            // Never the literal inheritance token "main" — that is config
            // vocabulary, not an answer to the operator's question.
            let provider = if role_cfg.provider == "main" {
                main_provider_name.clone()
            } else {
                role_cfg.provider.clone()
            };
            let model = resolver.resolve_role(role_name).map(|ep| ep.default_model);
            BuzzModelResolution {
                label: role_name.clone(),
                provider,
                model,
            }
        })
        .collect();
    roles.sort_by(|a, b| a.label.cmp(&b.label));

    let config_source = match scope {
        ConfigScope::Root => "the root config.yaml".to_string(),
        ConfigScope::Profile(name) => format!("the '{name}' profile's config.yaml"),
    };

    BuzzModelDisclosure::Resolved(BuzzRuntimeDisclosure {
        config_source,
        main,
        roles,
    })
}

/// D-10 sibling of `tools_config_api::check_tools_write_gate` /
/// `tools_credentials_api::check_credentials_write_gate` (module docs there
/// name this the established per-module duplication pattern in this phase —
/// `resolve_scope_target`/`save_scoped` are the two helpers this plan
/// promotes to shared, module doc's promotion note explains why this one
/// stays a sibling rather than a third promotion). Fail-closed: reads
/// `security.web_config_write_enabled` from a FRESH ROOT `Config::load()`
/// regardless of the scope being edited.
#[cfg(not(target_arch = "wasm32"))]
fn check_buzz_write_gate() -> Result<(), String> {
    let root_config =
        ironhermes_core::config::Config::load().map_err(|e| format!("Config load failed: {e}"))?;
    if !root_config.security.web_config_write_enabled {
        return Err("Config writes are disabled".to_string());
    }
    Ok(())
}

/// Pure(-ish) core of [`set_buzz_enabled`] — Task 1's one working field.
/// Staged-write order: resolve scope (fresh disk read) -> gate check ->
/// read-modify-write the EXISTING map entry (creating it from `Default`
/// only when genuinely absent, so `#[serde(flatten)] extra` and every
/// sibling platform entry survive) -> atomic save -> re-read fresh from
/// disk so the returned DTO reflects what is actually on disk.
#[cfg(not(target_arch = "wasm32"))]
async fn set_buzz_enabled_impl(
    scope: ConfigScope,
    enabled: bool,
) -> Result<BuzzPlatformView, String> {
    let (mut config, target) = crate::server::tools_config_api::resolve_scope_target(&scope)?;
    check_buzz_write_gate()?;

    let mut platform = config
        .gateway
        .platforms
        .get(BUZZ_PLATFORM_KEY)
        .cloned()
        .unwrap_or_default();
    platform.enabled = enabled;
    config
        .gateway
        .platforms
        .insert(BUZZ_PLATFORM_KEY.to_string(), platform);
    crate::server::tools_config_api::save_scoped(&config, &target)?;

    let (reread, _reread_target) = crate::server::tools_config_api::resolve_scope_target(&scope)?;
    // Computed from the RE-READ config, not the pre-write one, so the
    // returned view is consistent with what is actually on disk.
    let disclosure = resolve_buzz_model_disclosure(&reread, &scope);
    Ok(build_buzz_view(
        reread.gateway.platforms.get(BUZZ_PLATFORM_KEY),
        disclosure,
    ))
}

// =============================================================================
// #[server] fns — thin wrappers, dioxus fullstack codec split.
// =============================================================================

/// Read the Buzz platform state for `scope` — never errors on an absent
/// `buzz:` block; that is the `configured: false` answer, not a failure.
#[server]
pub async fn get_buzz_platform_view(scope: ConfigScope) -> Result<BuzzPlatformView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (config, _target) = crate::server::tools_config_api::resolve_scope_target(&scope)
            .map_err(ServerFnError::new)?;
        let disclosure = resolve_buzz_model_disclosure(&config, &scope);
        Ok(build_buzz_view(
            config.gateway.platforms.get(BUZZ_PLATFORM_KEY),
            disclosure,
        ))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scope;
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Write the Buzz `enabled` flag for `scope` — Task 1's one working
/// end-to-end field. Refuses before touching config when the write gate is
/// closed; every other field of the map entry (and every sibling platform
/// entry, and `#[serde(flatten)] extra`) survives untouched.
#[server]
pub async fn set_buzz_enabled(
    scope: ConfigScope,
    enabled: bool,
) -> Result<BuzzPlatformView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        set_buzz_enabled_impl(scope, enabled)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, enabled);
        unreachable!("server fn body never runs on the wasm client")
    }
}

// =============================================================================
// Task 2 — staged edit: whitelist, relay_url, channels (D-11: one validated,
// gated, atomic save; channel_trust is NEVER accepted here — Task 2's
// prohibition, see module doc and the threat register's T-48.2-12-05).
// =============================================================================

/// A list entry (whitelist or channel) is capped at this many characters —
/// generous against a hex pubkey (64 chars) or a bech32 npub (~63 chars),
/// small enough that a paste-accident cannot smuggle a multi-megabyte value
/// into config.yaml through a single entry.
const MAX_ENTRY_LEN: usize = 512;

/// Either list (whitelist or channels) is capped at this many entries — a
/// sane bound so a paste accident cannot write a multi-megabyte config.
const MAX_LIST_ENTRIES: usize = 1000;

/// The staged-write payload for [`set_buzz_edit`] — deliberately touches
/// nothing but whitelist/relay_url/channels; `enabled` has its own fn
/// (Task 1) and `channel_trust` has NO write fn anywhere in this module
/// (Task 2 prohibition — read-only by construction).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BuzzEditPayload {
    pub whitelist: Vec<String>,
    /// Empty string means "no relay configured" and normalizes to `None` —
    /// the staged form's text input has no separate representation for
    /// "unset" vs. "empty string typed".
    pub relay_url: Option<String>,
    pub channels: Vec<String>,
}

/// The whitelist's own contract (`PlatformGatewayConfig::whitelist`'s doc
/// comment, `ironhermes-core/src/config.rs`): an empty list denies every
/// sender. `buzz_section.rs` renders this as an ACTIVE warning — never a
/// passive footnote — whenever this predicate is true, including on the
/// STAGED (not yet saved) list, so an operator clearing the list to "start
/// fresh" sees the consequence before they click SAVE.
pub fn whitelist_denies_all_senders(whitelist: &[String]) -> bool {
    whitelist.is_empty()
}

/// Validated + normalized (trimmed, de-duplicated) form of a staged edit —
/// never constructed except by [`validate_and_normalize_buzz_edit`], so
/// every field this struct carries has already passed every check.
#[derive(Debug)]
#[cfg(not(target_arch = "wasm32"))]
struct NormalizedBuzzEdit {
    whitelist: Vec<String>,
    relay_url: Option<String>,
    channels: Vec<String>,
}

/// Validate + normalize one list field (whitelist or channels). No disk
/// I/O; concatenates every problem found rather than stopping at the
/// first, mirroring `tools_config_api::validate_tools_settings`'s
/// "an operator with two typos sees both in one round trip" precedent.
///
/// Error messages name the field and the entry's INDEX, never the entry's
/// own text (T-48.2-12-10 — a validator must never echo an unbounded
/// input back; an over-length or malformed paste is exactly the input
/// this rule exists to keep out of the error channel).
#[cfg(not(target_arch = "wasm32"))]
fn validate_and_normalize_entries(
    field_name: &str,
    entries: &[String],
) -> Result<Vec<String>, Vec<String>> {
    let mut errors = Vec::new();
    if entries.len() > MAX_LIST_ENTRIES {
        errors.push(format!(
            "{field_name} has too many entries ({} of a {MAX_LIST_ENTRIES} maximum)",
            entries.len()
        ));
    }

    let mut normalized: Vec<String> = Vec::new();
    for (i, raw) in entries.iter().enumerate() {
        // Length check BEFORE trim — an oversized entry is rejected on its
        // own length regardless of surrounding whitespace.
        if raw.len() > MAX_ENTRY_LEN {
            errors.push(format!(
                "{field_name} entry {i} exceeds {MAX_ENTRY_LEN} characters"
            ));
            continue;
        }
        if raw.contains('\n') || raw.contains('\r') {
            errors.push(format!("{field_name} entry {i} must not contain a newline"));
            continue;
        }
        let trimmed = raw.trim().to_string();
        if trimmed.is_empty() {
            errors.push(format!(
                "{field_name} entry {i} must not be empty or whitespace-only"
            ));
            continue;
        }
        // De-duplicate on the TRIMMED form — the whitelist's own contract
        // already merges the integer/string spellings of one value into
        // one authorization (`deserialize_whitelist`); this merges
        // whitespace-variant duplicates the same way, at the same layer.
        if !normalized.contains(&trimmed) {
            normalized.push(trimmed);
        }
    }

    if errors.is_empty() {
        Ok(normalized)
    } else {
        Err(errors)
    }
}

/// Validate a relay URL: must be an absolute `ws://` or `wss://` URL with a
/// non-empty host. A Nostr relay is a WebSocket endpoint — accepting an
/// `http`/`https` URL here would produce a gateway that fails at connect
/// time with a confusing error far from this form. No `url` crate
/// dependency is added for this (out of this plan's `files_modified`
/// scope) — the check is a bounded manual scheme/host split, not a full
/// RFC 3986 parse.
#[cfg(not(target_arch = "wasm32"))]
fn validate_relay_url(url: &str) -> Result<(), String> {
    if url.len() > MAX_ENTRY_LEN {
        return Err(format!("relay_url exceeds {MAX_ENTRY_LEN} characters"));
    }
    if url.contains('\n') || url.contains('\r') {
        return Err("relay_url must not contain a newline".to_string());
    }
    let after_scheme = url
        .strip_prefix("wss://")
        .or_else(|| url.strip_prefix("ws://"));
    let Some(after_scheme) = after_scheme else {
        return Err("relay_url must be an absolute ws:// or wss:// URL".to_string());
    };
    let host_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    if after_scheme[..host_end].trim().is_empty() {
        return Err("relay_url must include a host after the scheme".to_string());
    }
    Ok(())
}

/// Pure core: validate every field, normalize the list fields, and turn an
/// empty-after-trim `relay_url` into `None` (module doc on
/// [`BuzzEditPayload::relay_url`]). No disk I/O — every branch below is
/// directly unit-testable without a `Config`.
#[cfg(not(target_arch = "wasm32"))]
fn validate_and_normalize_buzz_edit(
    payload: &BuzzEditPayload,
) -> Result<NormalizedBuzzEdit, Vec<String>> {
    let mut errors = Vec::new();

    let whitelist = match validate_and_normalize_entries("whitelist", &payload.whitelist) {
        Ok(v) => v,
        Err(e) => {
            errors.extend(e);
            Vec::new()
        }
    };
    let channels = match validate_and_normalize_entries("channels", &payload.channels) {
        Ok(v) => v,
        Err(e) => {
            errors.extend(e);
            Vec::new()
        }
    };
    let relay_url = match payload.relay_url.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(trimmed) => match validate_relay_url(trimmed) {
            Ok(()) => Some(trimmed.to_string()),
            Err(e) => {
                errors.push(e);
                None
            }
        },
    };

    if errors.is_empty() {
        Ok(NormalizedBuzzEdit {
            whitelist,
            relay_url,
            channels,
        })
    } else {
        Err(errors)
    }
}

/// Pure(-ish) core of [`set_buzz_edit`]. Staged-write order: validate (no
/// disk I/O — a rejected field aborts here, before anything is read or
/// written) -> resolve scope (fresh disk read) -> gate check ->
/// read-modify-write the EXISTING map entry (channel_trust and every other
/// field of the entry survives — this fn never touches them) -> atomic
/// save -> re-read fresh from disk.
#[cfg(not(target_arch = "wasm32"))]
async fn set_buzz_edit_impl(
    scope: ConfigScope,
    payload: BuzzEditPayload,
) -> Result<BuzzPlatformView, Vec<String>> {
    let normalized = validate_and_normalize_buzz_edit(&payload)?;

    let (mut config, target) =
        crate::server::tools_config_api::resolve_scope_target(&scope).map_err(|e| vec![e])?;
    check_buzz_write_gate().map_err(|e| vec![e])?;

    let mut platform = config
        .gateway
        .platforms
        .get(BUZZ_PLATFORM_KEY)
        .cloned()
        .unwrap_or_default();
    platform.whitelist = normalized.whitelist;
    platform.relay_url = normalized.relay_url;
    platform.channels = normalized.channels;
    config
        .gateway
        .platforms
        .insert(BUZZ_PLATFORM_KEY.to_string(), platform);
    crate::server::tools_config_api::save_scoped(&config, &target).map_err(|e| vec![e])?;

    let (reread, _reread_target) =
        crate::server::tools_config_api::resolve_scope_target(&scope).map_err(|e| vec![e])?;
    // Computed from the RE-READ config, not the pre-write one, so the
    // returned view is consistent with what is actually on disk.
    let disclosure = resolve_buzz_model_disclosure(&reread, &scope);
    Ok(build_buzz_view(
        reread.gateway.platforms.get(BUZZ_PLATFORM_KEY),
        disclosure,
    ))
}

/// Staged-write commit for whitelist/relay_url/channels (Task 2, D-11) —
/// one validated, gated, atomic save. Never accepts `channel_trust` — that
/// field has no write fn anywhere in this module (Task 2 prohibition).
#[server]
pub async fn set_buzz_edit(
    scope: ConfigScope,
    payload: BuzzEditPayload,
) -> Result<BuzzPlatformView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        set_buzz_edit_impl(scope, payload)
            .await
            .map_err(|errors| ServerFnError::new(errors.join("; ")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, payload);
        unreachable!("server fn body never runs on the wasm client")
    }
}

// =============================================================================
// Phase 49.3 Plan 01 (tracer slice, D-06): the Telegram platform's read
// surface over `gateway.platforms["telegram"]` — modeled verbatim on the
// Buzz surface above (module doc's "DTO is an explicit allowlist"
// discipline applies identically here). Discord/Slack extend this same
// shape in later plans of this phase; this module owns the shared
// `resolve_scope_target`/`save_scoped`/`check_buzz_write_gate` pipeline
// every one of them reuses — see those plans' own module docs when they
// land.
// =============================================================================

/// The `gateway.platforms` map key this Telegram surface reads and writes.
/// Never hardcoded a second time below — every lookup goes through this
/// constant (mirrors [`BUZZ_PLATFORM_KEY`]).
const TELEGRAM_PLATFORM_KEY: &str = "telegram";

/// Explicit field allowlist over `gateway.platforms["telegram"]`
/// (`PlatformGatewayConfig`) — see module doc's "DTO is an explicit
/// allowlist" section. `configured` distinguishes "no `telegram:` block
/// exists yet" from "the block exists and is disabled", the same Task 1
/// distinction [`BuzzPlatformView::configured`] makes. `token_present` is a
/// presence flag derived from `PlatformGatewayConfig::token.is_some()` —
/// the token VALUE never reaches this DTO, and no field in this struct is
/// named `token`/`app_token`/`api_key` (enforced below by
/// [`tests::telegram_view_dto_carries_no_secret_bearing_field`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TelegramPlatformView {
    pub configured: bool,
    pub enabled: bool,
    /// Canonical cross-platform sender allowlist — same field/contract as
    /// [`BuzzPlatformView::whitelist`]. Empty = deny all.
    pub whitelist: Vec<String>,
    /// D-06 advanced tier: explicit Telegram home channel
    /// (`PlatformGatewayConfig::home_channel_id`'s own doc comment,
    /// `ironhermes-core/src/config.rs`).
    pub home_channel_id: Option<String>,
    /// D-06 advanced tier: session inactivity timeout in hours.
    pub session_timeout_hours: u64,
    /// D-06 advanced tier: maximum concurrent agent runs.
    pub max_concurrent_runs: usize,
    /// Presence-only flag for the bot token — never the token value itself.
    pub token_present: bool,
}

/// Pure builder — mirrors [`build_buzz_view`]'s "'not configured' is its
/// own answer" discipline: `entry` is `None` only when no `telegram:`
/// block exists in `gateway.platforms` at all, never derived from
/// `enabled` or any other field. The absent-block defaults for
/// `session_timeout_hours`/`max_concurrent_runs` come from
/// `PlatformGatewayConfig::default()` so an unconfigured card still shows
/// the values the gateway would actually use if the block were created.
/// No disk I/O; directly unit-testable.
#[cfg(not(target_arch = "wasm32"))]
fn build_telegram_view(
    entry: Option<&ironhermes_core::config::PlatformGatewayConfig>,
) -> TelegramPlatformView {
    match entry {
        Some(cfg) => TelegramPlatformView {
            configured: true,
            enabled: cfg.enabled,
            whitelist: cfg.whitelist.clone(),
            home_channel_id: cfg.home_channel_id.clone(),
            session_timeout_hours: cfg.session_timeout_hours,
            max_concurrent_runs: cfg.max_concurrent_runs,
            token_present: cfg.token.is_some(),
        },
        None => {
            let default_cfg = ironhermes_core::config::PlatformGatewayConfig::default();
            TelegramPlatformView {
                configured: false,
                enabled: false,
                whitelist: Vec::new(),
                home_channel_id: None,
                session_timeout_hours: default_cfg.session_timeout_hours,
                max_concurrent_runs: default_cfg.max_concurrent_runs,
                token_present: false,
            }
        }
    }
}

/// Read the Telegram platform state for `scope` — never errors on an
/// absent `telegram:` block; that is the `configured: false` answer, not a
/// failure. Mirrors [`get_buzz_platform_view`] exactly, including the
/// shared [`crate::server::tools_config_api::resolve_scope_target`] scope
/// resolution (root vs. a named profile's own `config.yaml`, never
/// conflated).
#[server]
pub async fn get_telegram_platform_view(
    scope: ConfigScope,
) -> Result<TelegramPlatformView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (config, _target) = crate::server::tools_config_api::resolve_scope_target(&scope)
            .map_err(ServerFnError::new)?;
        Ok(build_telegram_view(
            config.gateway.platforms.get(TELEGRAM_PLATFORM_KEY),
        ))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scope;
        unreachable!("server fn body never runs on the wasm client")
    }
}

// =============================================================================
// Phase 49.3 Plan 02: Telegram instant-write `enabled` toggle — mirrors
// [`set_buzz_enabled_impl`]/[`set_buzz_enabled`] exactly, including reuse of
// [`check_buzz_write_gate`] (the gate is generic over every
// `gateway.platforms.*` write, not Buzz-specific despite its name — see
// that fn's own doc comment; module doc's "D-10 sibling" note explains why
// this plan does not promote/rename it a second time).
// =============================================================================

/// Pure(-ish) core of [`set_telegram_enabled`]. Staged-write order: resolve
/// scope (fresh disk read) -> gate check -> read-modify-write the EXISTING
/// map entry (creating it from `Default` only when genuinely absent, so
/// every sibling platform entry and `#[serde(flatten)] extra` survive) ->
/// atomic save -> re-read fresh from disk so the returned DTO reflects what
/// is actually on disk.
#[cfg(not(target_arch = "wasm32"))]
async fn set_telegram_enabled_impl(
    scope: ConfigScope,
    enabled: bool,
) -> Result<TelegramPlatformView, String> {
    let (mut config, target) = crate::server::tools_config_api::resolve_scope_target(&scope)?;
    check_buzz_write_gate()?;

    let mut platform = config
        .gateway
        .platforms
        .get(TELEGRAM_PLATFORM_KEY)
        .cloned()
        .unwrap_or_default();
    platform.enabled = enabled;
    config
        .gateway
        .platforms
        .insert(TELEGRAM_PLATFORM_KEY.to_string(), platform);
    crate::server::tools_config_api::save_scoped(&config, &target)?;

    let (reread, _reread_target) = crate::server::tools_config_api::resolve_scope_target(&scope)?;
    Ok(build_telegram_view(
        reread.gateway.platforms.get(TELEGRAM_PLATFORM_KEY),
    ))
}

/// Write the Telegram `enabled` flag for `scope` — the tracer's one
/// working end-to-end write field (D-11 instant write; D-09 no
/// auto-restart — the caller renders the staged-apply pill, this fn never
/// touches the gateway process). Refuses before touching config when the
/// write gate is closed; every other field of the map entry (and every
/// sibling platform entry) survives untouched. Mirrors [`set_buzz_enabled`].
#[server]
pub async fn set_telegram_enabled(
    scope: ConfigScope,
    enabled: bool,
) -> Result<TelegramPlatformView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        set_telegram_enabled_impl(scope, enabled)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, enabled);
        unreachable!("server fn body never runs on the wasm client")
    }
}

// =============================================================================
// Phase 49.3 Plan 03 (D-06): Discord/Slack DTOs + get/set_enabled/set_edit,
// plus Telegram's own staged `set_telegram_edit` (advanced tier). Every fn
// below is copied verbatim from the Buzz/Telegram template above, substituting
// only the map key and the field set — see module doc's "DTO is an explicit
// allowlist" discipline, which applies identically here.
// =============================================================================

/// The `gateway.platforms` map key the Discord surface reads and writes.
const DISCORD_PLATFORM_KEY: &str = "discord";

/// The `gateway.platforms` map key the Slack surface reads and writes.
const SLACK_PLATFORM_KEY: &str = "slack";

/// Explicit field allowlist over `gateway.platforms["discord"]`
/// (`PlatformGatewayConfig`) — mirrors [`TelegramPlatformView`] exactly.
/// `token_present` is a presence flag derived from `token.is_some()` — the
/// token VALUE never reaches this DTO (enforced by
/// [`tests::discord_view_dto_carries_no_secret_bearing_field`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DiscordPlatformView {
    pub configured: bool,
    pub enabled: bool,
    /// Canonical cross-platform sender allowlist — same field/contract as
    /// [`BuzzPlatformView::whitelist`]. Discord's own adapter parses each
    /// entry as a u64 snowflake ID at the runner boundary
    /// (`ironhermes-gateway/src/runner.rs` ~:1285); this DTO carries the
    /// same untyped `Vec<String>` every platform shares — no numeric
    /// coercion happens here.
    pub whitelist: Vec<String>,
    /// D-06 advanced tier: explicit Discord home channel.
    pub home_channel_id: Option<String>,
    /// D-06 advanced tier: session inactivity timeout in hours.
    pub session_timeout_hours: u64,
    /// D-06 advanced tier: maximum concurrent agent runs.
    pub max_concurrent_runs: usize,
    /// Presence-only flag for the bot token — never the token value itself.
    pub token_present: bool,
}

/// Explicit field allowlist over `gateway.platforms["slack"]`
/// (`PlatformGatewayConfig`) — mirrors [`TelegramPlatformView`], plus
/// `app_token_present` for Slack's Socket Mode app-level token (`xapp-`).
/// Neither `token` nor `app_token` VALUES ever reach this DTO (enforced by
/// [`tests::slack_view_dto_carries_no_secret_bearing_field`] and
/// [`tests::slack_view_dto_never_carries_the_app_token_value`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SlackPlatformView {
    pub configured: bool,
    pub enabled: bool,
    /// Canonical cross-platform sender allowlist — same field/contract as
    /// [`BuzzPlatformView::whitelist`].
    pub whitelist: Vec<String>,
    /// D-06 advanced tier: explicit Slack home channel.
    pub home_channel_id: Option<String>,
    /// D-06 advanced tier: session inactivity timeout in hours.
    pub session_timeout_hours: u64,
    /// D-06 advanced tier: maximum concurrent agent runs.
    pub max_concurrent_runs: usize,
    /// Presence-only flag for the bot token — never the token value itself.
    pub token_present: bool,
    /// Presence-only flag for the Socket Mode app-level token (`xapp-`) —
    /// never the value itself.
    pub app_token_present: bool,
}

/// Pure builder — mirrors [`build_telegram_view`]'s "'not configured' is
/// its own answer" discipline. No disk I/O; directly unit-testable.
#[cfg(not(target_arch = "wasm32"))]
fn build_discord_view(
    entry: Option<&ironhermes_core::config::PlatformGatewayConfig>,
) -> DiscordPlatformView {
    match entry {
        Some(cfg) => DiscordPlatformView {
            configured: true,
            enabled: cfg.enabled,
            whitelist: cfg.whitelist.clone(),
            home_channel_id: cfg.home_channel_id.clone(),
            session_timeout_hours: cfg.session_timeout_hours,
            max_concurrent_runs: cfg.max_concurrent_runs,
            token_present: cfg.token.is_some(),
        },
        None => {
            let default_cfg = ironhermes_core::config::PlatformGatewayConfig::default();
            DiscordPlatformView {
                configured: false,
                enabled: false,
                whitelist: Vec::new(),
                home_channel_id: None,
                session_timeout_hours: default_cfg.session_timeout_hours,
                max_concurrent_runs: default_cfg.max_concurrent_runs,
                token_present: false,
            }
        }
    }
}

/// Pure builder — mirrors [`build_telegram_view`], plus
/// `app_token_present` derived from `app_token.is_some()`. No disk I/O;
/// directly unit-testable.
#[cfg(not(target_arch = "wasm32"))]
fn build_slack_view(
    entry: Option<&ironhermes_core::config::PlatformGatewayConfig>,
) -> SlackPlatformView {
    match entry {
        Some(cfg) => SlackPlatformView {
            configured: true,
            enabled: cfg.enabled,
            whitelist: cfg.whitelist.clone(),
            home_channel_id: cfg.home_channel_id.clone(),
            session_timeout_hours: cfg.session_timeout_hours,
            max_concurrent_runs: cfg.max_concurrent_runs,
            token_present: cfg.token.is_some(),
            app_token_present: cfg.app_token.is_some(),
        },
        None => {
            let default_cfg = ironhermes_core::config::PlatformGatewayConfig::default();
            SlackPlatformView {
                configured: false,
                enabled: false,
                whitelist: Vec::new(),
                home_channel_id: None,
                session_timeout_hours: default_cfg.session_timeout_hours,
                max_concurrent_runs: default_cfg.max_concurrent_runs,
                token_present: false,
                app_token_present: false,
            }
        }
    }
}

/// Read the Discord platform state for `scope` — never errors on an absent
/// `discord:` block. Mirrors [`get_telegram_platform_view`] exactly.
#[server]
pub async fn get_discord_platform_view(
    scope: ConfigScope,
) -> Result<DiscordPlatformView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (config, _target) = crate::server::tools_config_api::resolve_scope_target(&scope)
            .map_err(ServerFnError::new)?;
        Ok(build_discord_view(
            config.gateway.platforms.get(DISCORD_PLATFORM_KEY),
        ))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scope;
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Read the Slack platform state for `scope` — never errors on an absent
/// `slack:` block. Mirrors [`get_telegram_platform_view`] exactly.
#[server]
pub async fn get_slack_platform_view(scope: ConfigScope) -> Result<SlackPlatformView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (config, _target) = crate::server::tools_config_api::resolve_scope_target(&scope)
            .map_err(ServerFnError::new)?;
        Ok(build_slack_view(
            config.gateway.platforms.get(SLACK_PLATFORM_KEY),
        ))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scope;
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Pure(-ish) core of [`set_discord_enabled`]. Mirrors
/// [`set_telegram_enabled_impl`] exactly.
#[cfg(not(target_arch = "wasm32"))]
async fn set_discord_enabled_impl(
    scope: ConfigScope,
    enabled: bool,
) -> Result<DiscordPlatformView, String> {
    let (mut config, target) = crate::server::tools_config_api::resolve_scope_target(&scope)?;
    check_buzz_write_gate()?;

    let mut platform = config
        .gateway
        .platforms
        .get(DISCORD_PLATFORM_KEY)
        .cloned()
        .unwrap_or_default();
    platform.enabled = enabled;
    config
        .gateway
        .platforms
        .insert(DISCORD_PLATFORM_KEY.to_string(), platform);
    crate::server::tools_config_api::save_scoped(&config, &target)?;

    let (reread, _reread_target) = crate::server::tools_config_api::resolve_scope_target(&scope)?;
    Ok(build_discord_view(
        reread.gateway.platforms.get(DISCORD_PLATFORM_KEY),
    ))
}

/// Write the Discord `enabled` flag for `scope`. Mirrors
/// [`set_telegram_enabled`] exactly.
#[server]
pub async fn set_discord_enabled(
    scope: ConfigScope,
    enabled: bool,
) -> Result<DiscordPlatformView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        set_discord_enabled_impl(scope, enabled)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, enabled);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Pure(-ish) core of [`set_slack_enabled`]. Mirrors
/// [`set_telegram_enabled_impl`] exactly.
#[cfg(not(target_arch = "wasm32"))]
async fn set_slack_enabled_impl(
    scope: ConfigScope,
    enabled: bool,
) -> Result<SlackPlatformView, String> {
    let (mut config, target) = crate::server::tools_config_api::resolve_scope_target(&scope)?;
    check_buzz_write_gate()?;

    let mut platform = config
        .gateway
        .platforms
        .get(SLACK_PLATFORM_KEY)
        .cloned()
        .unwrap_or_default();
    platform.enabled = enabled;
    config
        .gateway
        .platforms
        .insert(SLACK_PLATFORM_KEY.to_string(), platform);
    crate::server::tools_config_api::save_scoped(&config, &target)?;

    let (reread, _reread_target) = crate::server::tools_config_api::resolve_scope_target(&scope)?;
    Ok(build_slack_view(
        reread.gateway.platforms.get(SLACK_PLATFORM_KEY),
    ))
}

/// Write the Slack `enabled` flag for `scope`. Mirrors
/// [`set_telegram_enabled`] exactly.
#[server]
pub async fn set_slack_enabled(
    scope: ConfigScope,
    enabled: bool,
) -> Result<SlackPlatformView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        set_slack_enabled_impl(scope, enabled)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, enabled);
        unreachable!("server fn body never runs on the wasm client")
    }
}

// =============================================================================
// Shared staged-edit payload for whitelist/home_channel_id/
// session_timeout_hours/max_concurrent_runs (D-06 primary + advanced
// tiers) — the SAME field set across Telegram/Discord/Slack, so one payload
// type and one validator serve all three `set_*_edit` fns below. NEVER
// touches `enabled` (its own fn, Task 1 shape) or `token`/`app_token` (no
// write fn anywhere in this module accepts either — the secret path is
// `set_gateway_secret`, plan 02).
// =============================================================================

/// The staged-write payload shared by [`set_telegram_edit`],
/// [`set_discord_edit`], and [`set_slack_edit`] — deliberately touches
/// nothing but whitelist/home_channel_id/session_timeout_hours/
/// max_concurrent_runs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ChatEditPayload {
    pub whitelist: Vec<String>,
    /// Empty string means "no home channel set" and normalizes to `None` —
    /// the staged form's text input has no separate representation for
    /// "unset" vs. "empty string typed" (mirrors [`BuzzEditPayload::relay_url`]).
    pub home_channel_id: Option<String>,
    pub session_timeout_hours: u64,
    pub max_concurrent_runs: usize,
}

/// Validated + normalized (trimmed) form of a staged chat-platform edit —
/// never constructed except by [`validate_and_normalize_chat_edit`].
#[derive(Debug)]
#[cfg(not(target_arch = "wasm32"))]
struct NormalizedChatEdit {
    whitelist: Vec<String>,
    home_channel_id: Option<String>,
    session_timeout_hours: u64,
    max_concurrent_runs: usize,
}

/// Pure core: validate every field, normalize the whitelist, and turn an
/// empty-after-trim `home_channel_id` into `None`. No disk I/O — every
/// branch below is directly unit-testable without a `Config`. Reuses
/// [`validate_and_normalize_entries`] verbatim for whitelist validation
/// (concatenates all problems in one round trip, names field+index, never
/// echoes the raw entry text — T-48.2-12-10).
#[cfg(not(target_arch = "wasm32"))]
fn validate_and_normalize_chat_edit(
    payload: &ChatEditPayload,
) -> Result<NormalizedChatEdit, Vec<String>> {
    let mut errors = Vec::new();

    let whitelist = match validate_and_normalize_entries("whitelist", &payload.whitelist) {
        Ok(v) => v,
        Err(e) => {
            errors.extend(e);
            Vec::new()
        }
    };

    let home_channel_id = match payload.home_channel_id.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(trimmed) => {
            if trimmed.len() > MAX_ENTRY_LEN {
                errors.push(format!(
                    "home_channel_id exceeds {MAX_ENTRY_LEN} characters"
                ));
                None
            } else if trimmed.contains('\n') || trimmed.contains('\r') {
                errors.push("home_channel_id must not contain a newline".to_string());
                None
            } else {
                Some(trimmed.to_string())
            }
        }
    };

    if errors.is_empty() {
        Ok(NormalizedChatEdit {
            whitelist,
            home_channel_id,
            session_timeout_hours: payload.session_timeout_hours,
            max_concurrent_runs: payload.max_concurrent_runs,
        })
    } else {
        Err(errors)
    }
}

/// Pure(-ish) core of [`set_telegram_edit`]. Staged-write order mirrors
/// [`set_buzz_edit_impl`]: validate (no disk I/O) -> resolve scope (fresh
/// disk read) -> gate check -> read-modify-write the EXISTING map entry
/// (`token` and every other field survives — this fn never touches it) ->
/// atomic save -> re-read fresh from disk.
#[cfg(not(target_arch = "wasm32"))]
async fn set_telegram_edit_impl(
    scope: ConfigScope,
    payload: ChatEditPayload,
) -> Result<TelegramPlatformView, Vec<String>> {
    let normalized = validate_and_normalize_chat_edit(&payload)?;

    let (mut config, target) =
        crate::server::tools_config_api::resolve_scope_target(&scope).map_err(|e| vec![e])?;
    check_buzz_write_gate().map_err(|e| vec![e])?;

    let mut platform = config
        .gateway
        .platforms
        .get(TELEGRAM_PLATFORM_KEY)
        .cloned()
        .unwrap_or_default();
    platform.whitelist = normalized.whitelist;
    platform.home_channel_id = normalized.home_channel_id;
    platform.session_timeout_hours = normalized.session_timeout_hours;
    platform.max_concurrent_runs = normalized.max_concurrent_runs;
    config
        .gateway
        .platforms
        .insert(TELEGRAM_PLATFORM_KEY.to_string(), platform);
    crate::server::tools_config_api::save_scoped(&config, &target).map_err(|e| vec![e])?;

    let (reread, _reread_target) =
        crate::server::tools_config_api::resolve_scope_target(&scope).map_err(|e| vec![e])?;
    Ok(build_telegram_view(
        reread.gateway.platforms.get(TELEGRAM_PLATFORM_KEY),
    ))
}

/// Staged-write commit for Telegram's whitelist/home_channel_id/
/// session_timeout_hours/max_concurrent_runs (D-06 primary + advanced
/// tiers). Never accepts `token` — that field has no write fn anywhere in
/// this module (the secret path is `set_gateway_secret`, plan 02).
#[server]
pub async fn set_telegram_edit(
    scope: ConfigScope,
    payload: ChatEditPayload,
) -> Result<TelegramPlatformView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        set_telegram_edit_impl(scope, payload)
            .await
            .map_err(|errors| ServerFnError::new(errors.join("; ")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, payload);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Pure(-ish) core of [`set_discord_edit`]. Mirrors
/// [`set_telegram_edit_impl`] exactly, substituting the map key.
#[cfg(not(target_arch = "wasm32"))]
async fn set_discord_edit_impl(
    scope: ConfigScope,
    payload: ChatEditPayload,
) -> Result<DiscordPlatformView, Vec<String>> {
    let normalized = validate_and_normalize_chat_edit(&payload)?;

    let (mut config, target) =
        crate::server::tools_config_api::resolve_scope_target(&scope).map_err(|e| vec![e])?;
    check_buzz_write_gate().map_err(|e| vec![e])?;

    let mut platform = config
        .gateway
        .platforms
        .get(DISCORD_PLATFORM_KEY)
        .cloned()
        .unwrap_or_default();
    platform.whitelist = normalized.whitelist;
    platform.home_channel_id = normalized.home_channel_id;
    platform.session_timeout_hours = normalized.session_timeout_hours;
    platform.max_concurrent_runs = normalized.max_concurrent_runs;
    config
        .gateway
        .platforms
        .insert(DISCORD_PLATFORM_KEY.to_string(), platform);
    crate::server::tools_config_api::save_scoped(&config, &target).map_err(|e| vec![e])?;

    let (reread, _reread_target) =
        crate::server::tools_config_api::resolve_scope_target(&scope).map_err(|e| vec![e])?;
    Ok(build_discord_view(
        reread.gateway.platforms.get(DISCORD_PLATFORM_KEY),
    ))
}

/// Staged-write commit for Discord's whitelist/home_channel_id/
/// session_timeout_hours/max_concurrent_runs. Never accepts `token`.
#[server]
pub async fn set_discord_edit(
    scope: ConfigScope,
    payload: ChatEditPayload,
) -> Result<DiscordPlatformView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        set_discord_edit_impl(scope, payload)
            .await
            .map_err(|errors| ServerFnError::new(errors.join("; ")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, payload);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Pure(-ish) core of [`set_slack_edit`]. Mirrors [`set_telegram_edit_impl`]
/// exactly, substituting the map key. Never touches `app_token` — Slack's
/// Socket Mode app-level token has no write fn anywhere in this module
/// (the secret path is `set_gateway_secret`, plan 02).
#[cfg(not(target_arch = "wasm32"))]
async fn set_slack_edit_impl(
    scope: ConfigScope,
    payload: ChatEditPayload,
) -> Result<SlackPlatformView, Vec<String>> {
    let normalized = validate_and_normalize_chat_edit(&payload)?;

    let (mut config, target) =
        crate::server::tools_config_api::resolve_scope_target(&scope).map_err(|e| vec![e])?;
    check_buzz_write_gate().map_err(|e| vec![e])?;

    let mut platform = config
        .gateway
        .platforms
        .get(SLACK_PLATFORM_KEY)
        .cloned()
        .unwrap_or_default();
    platform.whitelist = normalized.whitelist;
    platform.home_channel_id = normalized.home_channel_id;
    platform.session_timeout_hours = normalized.session_timeout_hours;
    platform.max_concurrent_runs = normalized.max_concurrent_runs;
    config
        .gateway
        .platforms
        .insert(SLACK_PLATFORM_KEY.to_string(), platform);
    crate::server::tools_config_api::save_scoped(&config, &target).map_err(|e| vec![e])?;

    let (reread, _reread_target) =
        crate::server::tools_config_api::resolve_scope_target(&scope).map_err(|e| vec![e])?;
    Ok(build_slack_view(
        reread.gateway.platforms.get(SLACK_PLATFORM_KEY),
    ))
}

/// Staged-write commit for Slack's whitelist/home_channel_id/
/// session_timeout_hours/max_concurrent_runs. Never accepts `token` or
/// `app_token`.
#[server]
pub async fn set_slack_edit(
    scope: ConfigScope,
    payload: ChatEditPayload,
) -> Result<SlackPlatformView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        set_slack_edit_impl(scope, payload)
            .await
            .map_err(|errors| ServerFnError::new(errors.join("; ")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, payload);
        unreachable!("server fn body never runs on the wasm client")
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use ironhermes_core::config::{
        ChannelTrust, Config, CustomProviderConfig, ModelRoleConfig, PlatformGatewayConfig,
        ProviderConfig, SecurityConfig,
    };

    /// A minimal hand-built disclosure for tests that exercise
    /// `build_buzz_view`'s entry-mapping logic and do not care about the
    /// disclosure's own content — keeps those tests asserting exactly what
    /// they asserted before this field was added.
    fn sample_model_disclosure() -> BuzzModelDisclosure {
        BuzzModelDisclosure::Resolved(BuzzRuntimeDisclosure {
            config_source: "the root config.yaml".to_string(),
            main: BuzzModelResolution {
                label: "main".to_string(),
                provider: "openrouter".to_string(),
                model: Some("test-model".to_string()),
            },
            roles: Vec::new(),
        })
    }

    // -------------------------------------------------------------------
    // build_buzz_view — pure mapping tests (T-48.2-12-01/07)
    // -------------------------------------------------------------------

    /// The absent-key answer (`configured: false`) is structurally distinct
    /// from the present-but-disabled answer — both otherwise report the
    /// same `enabled: false` and empty lists, so `configured` is the only
    /// signal that separates them (Task 1's core acceptance criterion).
    #[test]
    fn absent_key_answer_differs_from_disabled_but_present_answer() {
        let absent = build_buzz_view(None, sample_model_disclosure());
        assert!(
            !absent.configured,
            "no buzz: block must report configured: false"
        );
        assert!(!absent.enabled);

        let present_disabled = PlatformGatewayConfig::default();
        let present = build_buzz_view(Some(&present_disabled), sample_model_disclosure());
        assert!(
            present.configured,
            "an explicit buzz: block must report configured: true"
        );
        assert!(!present.enabled);

        assert_ne!(
            absent, present,
            "absent and present-but-disabled must never collapse into the same DTO"
        );
    }

    #[test]
    fn present_entry_carries_its_real_fields_into_the_dto() {
        let cfg = PlatformGatewayConfig {
            enabled: true,
            whitelist: vec!["abc123".to_string()],
            relay_url: Some("wss://relay.example".to_string()),
            channels: vec!["chan-1".to_string()],
            channel_trust: ChannelTrust::Open,
            ..Default::default()
        };
        let view = build_buzz_view(Some(&cfg), sample_model_disclosure());
        assert!(view.configured);
        assert!(view.enabled);
        assert_eq!(view.whitelist, vec!["abc123".to_string()]);
        assert_eq!(view.relay_url, Some("wss://relay.example".to_string()));
        assert_eq!(view.channels, vec!["chan-1".to_string()]);
        assert_eq!(view.channel_trust, "open");
    }

    #[test]
    fn channel_trust_label_covers_both_variants() {
        assert_eq!(channel_trust_label(ChannelTrust::Closed), "closed");
        assert_eq!(channel_trust_label(ChannelTrust::Open), "open");
    }

    // -------------------------------------------------------------------
    // DTO shape — T-48.2-12-01 enforcing test
    // -------------------------------------------------------------------

    /// Recursively walk a serialized JSON value and assert none of the
    /// secret-bearing key names appears at ANY nesting depth — a top-level
    /// `contains_key` check would not see into `model_disclosure`'s nested
    /// object (T-48.2-14-01).
    fn assert_no_secret_key_at_any_depth(value: &serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for forbidden in ["token", "app_token", "api_key"] {
                    assert!(
                        !map.contains_key(forbidden),
                        "serialized DTO must never carry a field named '{forbidden}' at any nesting depth"
                    );
                }
                for v in map.values() {
                    assert_no_secret_key_at_any_depth(v);
                }
            }
            serde_json::Value::Array(items) => {
                for v in items {
                    assert_no_secret_key_at_any_depth(v);
                }
            }
            _ => {}
        }
    }

    /// The serialized DTO must never carry a field named after any of
    /// `PlatformGatewayConfig`'s secret-bearing fields — the property this
    /// module's entire design exists to guarantee (T-48.2-12-01,
    /// T-48.2-14-01). Extended in Plan 14 to construct a view WITH a
    /// populated `model_disclosure` (a nested object) and walk the
    /// serialized JSON recursively rather than checking only the top level.
    #[test]
    fn buzz_platform_view_dto_carries_no_secret_bearing_field() {
        let disclosure = BuzzModelDisclosure::Resolved(BuzzRuntimeDisclosure {
            config_source: "the root config.yaml".to_string(),
            main: BuzzModelResolution {
                label: "main".to_string(),
                provider: "openrouter".to_string(),
                model: Some("test-model".to_string()),
            },
            roles: vec![BuzzModelResolution {
                label: "summarization".to_string(),
                provider: "anthropic".to_string(),
                model: Some("claude-haiku".to_string()),
            }],
        });
        let view = BuzzPlatformView {
            configured: true,
            enabled: true,
            whitelist: vec!["abc".to_string()],
            relay_url: Some("wss://relay.example".to_string()),
            channels: vec!["chan".to_string()],
            channel_trust: "closed".to_string(),
            model_disclosure: disclosure,
        };
        let json = serde_json::to_value(&view).expect("DTO must serialize");
        assert_no_secret_key_at_any_depth(&json);
    }

    /// A literal `providers.<main>.api_key` value in config.yaml must never
    /// reach the serialized DTO — the strict resolver constructor reduces
    /// key presence but does not eliminate it (diagnosis fact 5); the
    /// allowlist copy in `resolve_buzz_model_disclosure` is the actual
    /// control.
    #[test]
    fn buzz_platform_view_dto_never_carries_a_literal_provider_api_key_marker() {
        let marker = "MARKER-SHOULD-NEVER-LEAK-7f3a9c1e";
        let mut cfg = Config {
            providers: [(
                "openrouter".to_string(),
                ProviderConfig {
                    api_key: Some(marker.to_string()),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Config::default()
        };
        cfg.model.provider = "openrouter".to_string();

        let disclosure = resolve_buzz_model_disclosure(&cfg, &ConfigScope::Root);
        let view = build_buzz_view(None, disclosure);
        let rendered = serde_json::to_string(&view).expect("DTO must serialize to a string");

        assert!(
            !rendered.contains(marker),
            "a literal providers.<main>.api_key value must never reach the serialized DTO"
        );
    }

    // -------------------------------------------------------------------
    // resolve_buzz_model_disclosure — Task 1 behavior tests (G-48.2-8)
    // -------------------------------------------------------------------

    /// The whole point of the gap: the disclosed main model must be the
    /// POST-overlay value (`providers.<main>.default_model`), never the
    /// pre-overlay `model.default` — a behavioral assertion on the
    /// returned value, not a source grep (a grep would pass even if the
    /// value arrived through a differently-named path).
    #[test]
    fn overlay_test_disclosure_shows_post_overlay_model_not_pre_overlay() {
        let mut cfg = Config {
            providers: [(
                "openrouter".to_string(),
                ProviderConfig {
                    default_model: Some("POST-OVERLAY-VALUE".to_string()),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Config::default()
        };
        cfg.model.provider = "openrouter".to_string();
        cfg.model.default = "PRE-OVERLAY-VALUE".to_string();

        let disclosure = resolve_buzz_model_disclosure(&cfg, &ConfigScope::Root);
        let BuzzModelDisclosure::Resolved(resolved) = disclosure else {
            panic!("expected a resolved disclosure for a valid config");
        };
        assert_eq!(
            resolved.main.model,
            Some("POST-OVERLAY-VALUE".to_string()),
            "disclosure must show the post-overlay provider value"
        );
        assert_ne!(
            resolved.main.model,
            Some("PRE-OVERLAY-VALUE".to_string()),
            "disclosure must NOT show the pre-overlay model.default value"
        );
    }

    /// A main provider name absent from every resolved endpoint (the
    /// resolver builds successfully with one on purpose — diagnosis fact 4)
    /// must produce the unresolvable variant, not a panic. The test
    /// completing at all is the assertion: the obvious accessor for this
    /// job (`resolve_for_main`) panics.
    #[test]
    fn unknown_main_provider_returns_unresolvable_without_panicking() {
        let mut cfg = Config::default();
        cfg.model.provider = "totally-unconfigured-provider".to_string();

        let disclosure = resolve_buzz_model_disclosure(&cfg, &ConfigScope::Root);
        assert!(
            matches!(disclosure, BuzzModelDisclosure::Unresolvable { .. }),
            "an unknown main provider must produce the unresolvable variant"
        );
    }

    /// Configured `model.roles` entries are disclosed with their own
    /// provider and model — a `"main"`-inheriting role names the REAL main
    /// provider (never the literal token `"main"`), and a role naming
    /// another configured provider carries that provider's own resolved
    /// model. Results are sorted by role name.
    #[test]
    fn configured_roles_are_disclosed_with_their_own_provider_and_model() {
        let mut cfg = Config::default();
        cfg.model.provider = "openrouter".to_string();
        cfg.model.roles.insert(
            "summarization".to_string(),
            ModelRoleConfig {
                provider: "main".to_string(),
                model: Some("summarization-override-model".to_string()),
            },
        );
        cfg.model.roles.insert(
            "vision".to_string(),
            ModelRoleConfig {
                provider: "anthropic".to_string(),
                model: None,
            },
        );

        let disclosure = resolve_buzz_model_disclosure(&cfg, &ConfigScope::Root);
        let BuzzModelDisclosure::Resolved(resolved) = disclosure else {
            panic!("expected a resolved disclosure for a valid config");
        };
        assert_eq!(resolved.roles.len(), 2);
        // Sorted by role name: "summarization" < "vision".
        assert_eq!(resolved.roles[0].label, "summarization");
        assert_eq!(
            resolved.roles[0].provider, "openrouter",
            "a \"main\"-inheriting role must carry the REAL main provider name, never the literal token \"main\""
        );
        assert_eq!(
            resolved.roles[0].model,
            Some("summarization-override-model".to_string())
        );
        assert_eq!(resolved.roles[1].label, "vision");
        assert_eq!(resolved.roles[1].provider, "anthropic");
        assert_eq!(
            resolved.roles[1].model,
            Some("claude-sonnet-4-20250514".to_string()),
            "a role with no model override takes the named provider's own default_model"
        );
    }

    /// A role naming a provider this config does not define is disclosed —
    /// not silently dropped — carrying its declared provider name and an
    /// absent model.
    #[test]
    fn role_naming_an_undefined_provider_is_disclosed_not_dropped() {
        let mut cfg = Config::default();
        cfg.model.provider = "openrouter".to_string();
        cfg.model.roles.insert(
            "kanban_judge".to_string(),
            ModelRoleConfig {
                provider: "not-a-configured-provider".to_string(),
                model: None,
            },
        );

        let disclosure = resolve_buzz_model_disclosure(&cfg, &ConfigScope::Root);
        let BuzzModelDisclosure::Resolved(resolved) = disclosure else {
            panic!("expected a resolved disclosure for a valid config");
        };
        assert_eq!(
            resolved.roles.len(),
            1,
            "the role must still appear, not vanish"
        );
        assert_eq!(resolved.roles[0].label, "kanban_judge");
        assert_eq!(resolved.roles[0].provider, "not-a-configured-provider");
        assert_eq!(resolved.roles[0].model, None);
    }

    /// Two DIFFERENT broken configs (distinct unsafe custom-provider
    /// base_urls, each carrying a distinct recognizable marker) both fail
    /// to build and must produce the SAME fixed reason string — a fixed,
    /// input-independent message cannot embed the input by construction
    /// (the same discipline `buzz_npub_api.rs`'s
    /// `derive_public_key_malformed_secret_returns_error_leaking_no_input_substring`
    /// uses). Neither marker may appear anywhere in the reason.
    #[test]
    fn two_different_broken_configs_produce_the_same_fixed_unresolvable_reason() {
        let marker_a = "unsafe-marker-alpha-9f2c";
        let marker_b = "unsafe-marker-beta-3e71";
        let mut cfg_a = Config::default();
        cfg_a.custom_providers.push(CustomProviderConfig {
            name: "custom-a".to_string(),
            base_url: format!("http://{marker_a}.example"),
            api_key: None,
            api_mode: None,
            default_model: None,
        });
        let mut cfg_b = Config::default();
        cfg_b.custom_providers.push(CustomProviderConfig {
            name: "custom-b".to_string(),
            base_url: format!("http://{marker_b}.example"),
            api_key: None,
            api_mode: None,
            default_model: None,
        });

        let disclosure_a = resolve_buzz_model_disclosure(&cfg_a, &ConfigScope::Root);
        let disclosure_b = resolve_buzz_model_disclosure(&cfg_b, &ConfigScope::Root);

        let (
            BuzzModelDisclosure::Unresolvable { reason: reason_a },
            BuzzModelDisclosure::Unresolvable { reason: reason_b },
        ) = (disclosure_a, disclosure_b)
        else {
            panic!("both configs carry an unsafe non-https/non-localhost base_url and must fail to build");
        };
        assert_eq!(
            reason_a, reason_b,
            "the disclosed reason must be identical regardless of the underlying build error"
        );
        assert!(!reason_a.contains(marker_a));
        assert!(!reason_a.contains(marker_b));
        assert!(!reason_b.contains(marker_a));
        assert!(!reason_b.contains(marker_b));
    }

    /// Root scope and a named profile scope produce different,
    /// human-readable `config_source` labels, and the profile label names
    /// the profile (D-08's scope dimension made legible).
    #[test]
    fn config_source_label_follows_scope() {
        let cfg = Config::default();

        let root_disclosure = resolve_buzz_model_disclosure(&cfg, &ConfigScope::Root);
        let profile_disclosure = resolve_buzz_model_disclosure(
            &cfg,
            &ConfigScope::Profile("my-bot-profile".to_string()),
        );

        let BuzzModelDisclosure::Resolved(root_resolved) = root_disclosure else {
            panic!("expected a resolved disclosure for the default config");
        };
        let BuzzModelDisclosure::Resolved(profile_resolved) = profile_disclosure else {
            panic!("expected a resolved disclosure for the default config");
        };
        assert_ne!(
            root_resolved.config_source, profile_resolved.config_source,
            "root and profile scope must produce different config_source labels"
        );
        assert!(
            profile_resolved.config_source.contains("my-bot-profile"),
            "the profile label must contain the profile name; got: {}",
            profile_resolved.config_source
        );
    }

    // -------------------------------------------------------------------
    // set_buzz_enabled_impl — gate / round-trip (disk-backed)
    // -------------------------------------------------------------------

    fn seeded_config(write_enabled: bool) -> Config {
        Config {
            security: SecurityConfig {
                web_config_write_enabled: write_enabled,
                web_process_control_enabled: false,
                remote_blueprint_run_enabled: false,
            },
            ..Config::default()
        }
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn write_refuses_when_gate_closed_before_touching_config() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let cfg = seeded_config(false);
        cfg.save().expect("seed root config.yaml");
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let result = set_buzz_enabled_impl(ConfigScope::Root, true).await;

        let after = std::fs::read(&config_path).expect("read config after refused write");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(
            result.is_err(),
            "write must be refused when the gate is closed"
        );
        assert_eq!(
            before, after,
            "a refused write must leave disk bytes unchanged"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn write_round_trip_preserves_sibling_platform_and_unknown_extra_key() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let mut cfg = seeded_config(true);
        let mut telegram = PlatformGatewayConfig {
            enabled: true,
            token: Some("telegram-token-should-never-move".to_string()),
            ..Default::default()
        };
        telegram
            .extra
            .insert("telegram_only_field".to_string(), serde_json::json!(42));
        cfg.gateway
            .platforms
            .insert("telegram".to_string(), telegram);

        let mut buzz = PlatformGatewayConfig {
            enabled: false,
            whitelist: vec!["existing-pubkey".to_string()],
            ..Default::default()
        };
        buzz.extra.insert(
            "an_unknown_buzz_key".to_string(),
            serde_json::json!("keep-me"),
        );
        cfg.gateway
            .platforms
            .insert(BUZZ_PLATFORM_KEY.to_string(), buzz);
        cfg.save().expect("seed root config.yaml");

        let result = set_buzz_enabled_impl(ConfigScope::Root, true).await;
        // Reload BEFORE clearing IRONHERMES_HOME — Config::load() resolves
        // against that env var, so clearing it first would read the real
        // operator home instead of this test's tempdir.
        let reloaded = ironhermes_core::config::Config::load();
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        let view = result.expect("write must succeed when the gate is open");
        assert!(view.enabled, "the enabled flag must reflect the write");
        assert_eq!(
            view.whitelist,
            vec!["existing-pubkey".to_string()],
            "an unrelated field on the SAME entry must survive the write"
        );

        let reloaded = reloaded.expect("reload saved config");
        let reloaded_telegram = reloaded
            .gateway
            .platforms
            .get("telegram")
            .expect("sibling platform entry must survive the buzz write");
        assert_eq!(
            reloaded_telegram.token,
            Some("telegram-token-should-never-move".to_string()),
            "a sibling platform's secret-bearing field must never be touched by a buzz write"
        );
        assert_eq!(
            reloaded_telegram.extra.get("telegram_only_field"),
            Some(&serde_json::json!(42)),
            "a sibling platform's unknown extra key must survive"
        );

        let reloaded_buzz = reloaded
            .gateway
            .platforms
            .get(BUZZ_PLATFORM_KEY)
            .expect("buzz entry must survive its own write");
        assert_eq!(
            reloaded_buzz.extra.get("an_unknown_buzz_key"),
            Some(&serde_json::json!("keep-me")),
            "an unknown key inside the buzz entry must survive a read-modify-write"
        );
    }

    // -------------------------------------------------------------------
    // Task 2 — whitelist_denies_all_senders (warning-state predicate)
    // -------------------------------------------------------------------

    #[test]
    fn whitelist_denies_all_senders_true_when_empty() {
        assert!(whitelist_denies_all_senders(&[]));
    }

    #[test]
    fn whitelist_denies_all_senders_false_when_non_empty() {
        assert!(!whitelist_denies_all_senders(&["abc123".to_string()]));
    }

    // -------------------------------------------------------------------
    // Task 2 — validate_and_normalize_entries
    // -------------------------------------------------------------------

    #[test]
    fn entries_trim_whitespace_and_deduplicate() {
        let result = validate_and_normalize_entries(
            "whitelist",
            &[
                "  abc123  ".to_string(),
                "abc123".to_string(),
                "def456".to_string(),
            ],
        );
        assert_eq!(
            result,
            Ok(vec!["abc123".to_string(), "def456".to_string()]),
            "trim + dedupe must collapse the whitespace-padded duplicate"
        );
    }

    #[test]
    fn entries_reject_empty_or_whitespace_only() {
        let result = validate_and_normalize_entries("whitelist", &["   ".to_string()]);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors[0].contains("empty or whitespace-only"));
        assert!(
            !errors[0].contains("   "),
            "error must not echo the rejected entry's own text (T-48.2-12-10)"
        );
    }

    #[test]
    fn entries_reject_newline_containing() {
        let result = validate_and_normalize_entries("channels", &["chan\n1".to_string()]);
        assert!(result.is_err());
        assert!(result.unwrap_err()[0].contains("newline"));
    }

    #[test]
    fn entries_reject_oversized_entry_without_echoing_it() {
        let huge = "a".repeat(MAX_ENTRY_LEN + 1);
        let result = validate_and_normalize_entries("whitelist", std::slice::from_ref(&huge));
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors[0].contains("exceeds"));
        assert!(
            !errors[0].contains(&huge),
            "error must not echo the oversized entry's own text (T-48.2-12-10)"
        );
    }

    #[test]
    fn entries_reject_when_list_too_long() {
        let too_many: Vec<String> = (0..MAX_LIST_ENTRIES + 1)
            .map(|i| format!("entry-{i}"))
            .collect();
        let result = validate_and_normalize_entries("channels", &too_many);
        assert!(result.is_err());
        assert!(result.unwrap_err()[0].contains("too many entries"));
    }

    #[test]
    fn empty_entries_list_is_a_legal_normalized_result() {
        assert_eq!(
            validate_and_normalize_entries("whitelist", &[]),
            Ok(Vec::new())
        );
    }

    // -------------------------------------------------------------------
    // Task 2 — validate_relay_url
    // -------------------------------------------------------------------

    #[test]
    fn relay_url_accepts_wss_and_ws() {
        assert!(validate_relay_url("wss://relay.example.com").is_ok());
        assert!(validate_relay_url("ws://relay.example.com:7777").is_ok());
    }

    #[test]
    fn relay_url_rejects_http_scheme() {
        let result = validate_relay_url("https://relay.example.com");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ws:// or wss://"));
    }

    #[test]
    fn relay_url_rejects_missing_host() {
        assert!(validate_relay_url("wss://").is_err());
        assert!(validate_relay_url("wss:///path-only").is_err());
    }

    #[test]
    fn relay_url_rejects_newline() {
        assert!(validate_relay_url("wss://relay.example\n.com").is_err());
    }

    // -------------------------------------------------------------------
    // Task 2 — validate_and_normalize_buzz_edit / set_buzz_edit_impl
    // -------------------------------------------------------------------

    #[test]
    fn edit_with_empty_relay_url_string_normalizes_to_none() {
        let payload = BuzzEditPayload {
            whitelist: vec![],
            relay_url: Some("   ".to_string()),
            channels: vec![],
        };
        let normalized = validate_and_normalize_buzz_edit(&payload).expect("must validate");
        assert_eq!(normalized.relay_url, None);
    }

    #[test]
    fn edit_rejects_when_any_field_invalid_and_names_all_problems() {
        let payload = BuzzEditPayload {
            whitelist: vec!["   ".to_string()],
            relay_url: Some("https://bad-scheme.example".to_string()),
            channels: vec![],
        };
        let result = validate_and_normalize_buzz_edit(&payload);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert_eq!(
            errors.len(),
            2,
            "both the whitelist entry problem AND the relay_url problem must be reported together"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn edit_with_empty_whitelist_saves_successfully_and_is_flagged_by_the_warning_predicate()
    {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let cfg = seeded_config(true);
        cfg.save().expect("seed root config.yaml");

        let payload = BuzzEditPayload {
            whitelist: vec![],
            relay_url: None,
            channels: vec!["chan-1".to_string()],
        };
        let result = set_buzz_edit_impl(ConfigScope::Root, payload).await;
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        let view = result.expect("an empty whitelist is a legal, meaningful configuration");
        assert!(view.whitelist.is_empty());
        assert!(
            whitelist_denies_all_senders(&view.whitelist),
            "the warning predicate must return true for the just-saved empty whitelist"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn edit_rejected_field_aborts_the_whole_save_with_nothing_written() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let cfg = seeded_config(true);
        cfg.save().expect("seed root config.yaml");
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        // whitelist is valid on its own; relay_url is not — the whole save
        // must abort, including the valid field.
        let payload = BuzzEditPayload {
            whitelist: vec!["a-valid-entry".to_string()],
            relay_url: Some("https://not-a-relay-scheme.example".to_string()),
            channels: vec![],
        };
        let result = set_buzz_edit_impl(ConfigScope::Root, payload).await;

        let after = std::fs::read(&config_path).expect("read config after rejected edit");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err());
        assert_eq!(
            before, after,
            "a rejected field must abort the ENTIRE save — nothing written, not even the valid field"
        );
    }

    // -------------------------------------------------------------------
    // Phase 49.3 Plan 01/02 — Telegram tracer slice
    // -------------------------------------------------------------------

    /// The absent-key answer (`configured: false`) is structurally distinct
    /// from the present-but-disabled answer — mirrors
    /// `absent_key_answer_differs_from_disabled_but_present_answer` for
    /// Buzz above.
    #[test]
    fn telegram_absent_key_answer_differs_from_disabled_but_present_answer() {
        let absent = build_telegram_view(None);
        assert!(
            !absent.configured,
            "no telegram: block must report configured: false"
        );
        assert!(!absent.enabled);
        assert!(!absent.token_present);

        let present_disabled = PlatformGatewayConfig::default();
        let present = build_telegram_view(Some(&present_disabled));
        assert!(
            present.configured,
            "an explicit telegram: block must report configured: true"
        );
        assert!(!present.enabled);

        assert_ne!(
            absent, present,
            "absent and present-but-disabled must never collapse into the same DTO"
        );
    }

    /// Task 1's behavior contract: a configured, enabled entry round-trips
    /// its real fields into the DTO, and `token_present` reflects presence
    /// only — never the token value.
    #[test]
    fn telegram_present_entry_carries_its_real_fields_into_the_dto() {
        let cfg = PlatformGatewayConfig {
            enabled: true,
            token: Some("secret-telegram-bot-token".to_string()),
            whitelist: vec!["123".to_string()],
            home_channel_id: Some("c".to_string()),
            ..Default::default()
        };
        let view = build_telegram_view(Some(&cfg));
        assert!(view.configured);
        assert!(view.enabled);
        assert_eq!(view.whitelist, vec!["123".to_string()]);
        assert_eq!(view.home_channel_id, Some("c".to_string()));
        assert!(
            view.token_present,
            "token_present must be true when a token is set"
        );
    }

    /// Recursively walk the serialized Telegram DTO and assert none of the
    /// secret-bearing key names appears at ANY nesting depth — the same
    /// walker `buzz_platform_view_dto_carries_no_secret_bearing_field`
    /// uses.
    #[test]
    fn telegram_view_dto_carries_no_secret_bearing_field() {
        let view = TelegramPlatformView {
            configured: true,
            enabled: true,
            whitelist: vec!["123".to_string()],
            home_channel_id: Some("c".to_string()),
            session_timeout_hours: 24,
            max_concurrent_runs: 8,
            token_present: true,
        };
        let json = serde_json::to_value(&view).expect("DTO must serialize");
        assert_no_secret_key_at_any_depth(&json);
    }

    /// Task 1's third behavior line: a profile-scope call reads the
    /// profile's own `config.yaml`, never root's.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn telegram_profile_scope_read_never_reads_root_config() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        // Seed ROOT with telegram DISABLED.
        let mut root_cfg = Config::default();
        root_cfg.gateway.platforms.insert(
            TELEGRAM_PLATFORM_KEY.to_string(),
            PlatformGatewayConfig {
                enabled: false,
                ..Default::default()
            },
        );
        root_cfg.save().expect("seed root config.yaml");

        // Seed a PROFILE with telegram ENABLED and a distinct whitelist.
        let profile_dir = crate::server::profile_api::profile_dir_for("tg-profile");
        std::fs::create_dir_all(&profile_dir).expect("create profile dir");
        let mut profile_cfg = Config::default();
        profile_cfg.gateway.platforms.insert(
            TELEGRAM_PLATFORM_KEY.to_string(),
            PlatformGatewayConfig {
                enabled: true,
                whitelist: vec!["profile-only-id".to_string()],
                ..Default::default()
            },
        );
        profile_cfg
            .save_to(&profile_dir.join("config.yaml"))
            .expect("seed profile config.yaml");

        let (resolved, _target) = crate::server::tools_config_api::resolve_scope_target(
            &ConfigScope::Profile("tg-profile".to_string()),
        )
        .expect("resolve profile scope");
        let view = build_telegram_view(resolved.gateway.platforms.get(TELEGRAM_PLATFORM_KEY));

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(
            view.enabled,
            "profile scope must read the PROFILE's telegram entry (enabled), not root's (disabled)"
        );
        assert_eq!(
            view.whitelist,
            vec!["profile-only-id".to_string()],
            "profile scope must read the profile's own whitelist, not root's"
        );
    }

    // -------------------------------------------------------------------
    // set_telegram_enabled_impl — gate / round-trip (disk-backed), mirrors
    // the Buzz enabled tests above.
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn telegram_write_refuses_when_gate_closed_before_touching_config() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let cfg = seeded_config(false);
        cfg.save().expect("seed root config.yaml");
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let result = set_telegram_enabled_impl(ConfigScope::Root, true).await;

        let after = std::fs::read(&config_path).expect("read config after refused write");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(
            result.is_err(),
            "write must be refused when the gate is closed"
        );
        assert_eq!(
            before, after,
            "a refused write must leave disk bytes unchanged"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn telegram_write_round_trip_preserves_sibling_platform_and_unknown_extra_key() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let mut cfg = seeded_config(true);
        let mut buzz = PlatformGatewayConfig {
            enabled: true,
            whitelist: vec!["buzz-pubkey-should-never-move".to_string()],
            ..Default::default()
        };
        buzz.extra.insert(
            "buzz_only_field".to_string(),
            serde_json::json!("keep-me"),
        );
        cfg.gateway
            .platforms
            .insert(BUZZ_PLATFORM_KEY.to_string(), buzz);

        let mut telegram = PlatformGatewayConfig {
            enabled: false,
            whitelist: vec!["existing-chat-id".to_string()],
            ..Default::default()
        };
        telegram.extra.insert(
            "an_unknown_telegram_key".to_string(),
            serde_json::json!("keep-me-too"),
        );
        cfg.gateway
            .platforms
            .insert(TELEGRAM_PLATFORM_KEY.to_string(), telegram);
        cfg.save().expect("seed root config.yaml");

        let result = set_telegram_enabled_impl(ConfigScope::Root, true).await;
        let reloaded = ironhermes_core::config::Config::load();
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        let view = result.expect("write must succeed when the gate is open");
        assert!(view.enabled, "the enabled flag must reflect the write");
        assert_eq!(
            view.whitelist,
            vec!["existing-chat-id".to_string()],
            "an unrelated field on the SAME entry must survive the write"
        );

        let reloaded = reloaded.expect("reload saved config");
        let reloaded_buzz = reloaded
            .gateway
            .platforms
            .get(BUZZ_PLATFORM_KEY)
            .expect("sibling platform entry must survive the telegram write");
        assert_eq!(
            reloaded_buzz.whitelist,
            vec!["buzz-pubkey-should-never-move".to_string()],
            "a sibling platform's field must never be touched by a telegram write"
        );
        assert_eq!(
            reloaded_buzz.extra.get("buzz_only_field"),
            Some(&serde_json::json!("keep-me")),
            "a sibling platform's unknown extra key must survive"
        );

        let reloaded_telegram = reloaded
            .gateway
            .platforms
            .get(TELEGRAM_PLATFORM_KEY)
            .expect("telegram entry must survive its own write");
        assert_eq!(
            reloaded_telegram.extra.get("an_unknown_telegram_key"),
            Some(&serde_json::json!("keep-me-too")),
            "an unknown key inside the telegram entry must survive a read-modify-write"
        );
    }

    // -------------------------------------------------------------------
    // Phase 49.3 Plan 03 — Discord/Slack DTOs + no-secret tests
    // -------------------------------------------------------------------

    #[test]
    fn discord_absent_key_answer_differs_from_disabled_but_present_answer() {
        let absent = build_discord_view(None);
        assert!(!absent.configured);
        assert!(!absent.enabled);
        assert!(!absent.token_present);

        let present_disabled = PlatformGatewayConfig::default();
        let present = build_discord_view(Some(&present_disabled));
        assert!(present.configured);
        assert!(!present.enabled);

        assert_ne!(absent, present);
    }

    #[test]
    fn discord_present_entry_carries_its_real_fields_into_the_dto() {
        let cfg = PlatformGatewayConfig {
            enabled: true,
            token: Some("secret-discord-bot-token".to_string()),
            whitelist: vec!["111222333".to_string()],
            home_channel_id: Some("c-discord".to_string()),
            ..Default::default()
        };
        let view = build_discord_view(Some(&cfg));
        assert!(view.configured);
        assert!(view.enabled);
        assert_eq!(view.whitelist, vec!["111222333".to_string()]);
        assert_eq!(view.home_channel_id, Some("c-discord".to_string()));
        assert!(view.token_present);
    }

    /// Recursively walk the serialized Discord DTO and assert none of the
    /// secret-bearing key names appears at ANY nesting depth.
    #[test]
    fn discord_view_dto_carries_no_secret_bearing_field() {
        let view = DiscordPlatformView {
            configured: true,
            enabled: true,
            whitelist: vec!["111222333".to_string()],
            home_channel_id: Some("c".to_string()),
            session_timeout_hours: 24,
            max_concurrent_runs: 8,
            token_present: true,
        };
        let json = serde_json::to_value(&view).expect("DTO must serialize");
        assert_no_secret_key_at_any_depth(&json);
    }

    #[test]
    fn slack_absent_key_answer_differs_from_disabled_but_present_answer() {
        let absent = build_slack_view(None);
        assert!(!absent.configured);
        assert!(!absent.enabled);
        assert!(!absent.token_present);
        assert!(!absent.app_token_present);

        let present_disabled = PlatformGatewayConfig::default();
        let present = build_slack_view(Some(&present_disabled));
        assert!(present.configured);
        assert!(!present.enabled);

        assert_ne!(absent, present);
    }

    #[test]
    fn slack_present_entry_carries_its_real_fields_into_the_dto() {
        let cfg = PlatformGatewayConfig {
            enabled: true,
            token: Some("xoxb-secret-slack-bot-token".to_string()),
            app_token: Some("xapp-secret-slack-app-token".to_string()),
            whitelist: vec!["U12345".to_string()],
            home_channel_id: Some("c-slack".to_string()),
            ..Default::default()
        };
        let view = build_slack_view(Some(&cfg));
        assert!(view.configured);
        assert!(view.enabled);
        assert_eq!(view.whitelist, vec!["U12345".to_string()]);
        assert_eq!(view.home_channel_id, Some("c-slack".to_string()));
        assert!(view.token_present);
        assert!(view.app_token_present);
    }

    /// Recursively walk the serialized Slack DTO and assert none of the
    /// secret-bearing key names appears at ANY nesting depth — the DTO
    /// carries `app_token_present: bool` only, never `app_token`.
    #[test]
    fn slack_view_dto_carries_no_secret_bearing_field() {
        let view = SlackPlatformView {
            configured: true,
            enabled: true,
            whitelist: vec!["U12345".to_string()],
            home_channel_id: Some("c".to_string()),
            session_timeout_hours: 24,
            max_concurrent_runs: 8,
            token_present: true,
            app_token_present: true,
        };
        let json = serde_json::to_value(&view).expect("DTO must serialize");
        assert_no_secret_key_at_any_depth(&json);
    }

    /// Dedicated assertion (beyond the generic recursive walk above) that
    /// the actual `xapp-` app-token VALUE never reaches the serialized
    /// Slack DTO — the DTO carries `app_token_present: true` only.
    #[test]
    fn slack_view_dto_never_carries_the_app_token_value() {
        const APP_TOKEN_MARKER: &str = "xapp-CANARY-should-never-leak-9f2c";
        let cfg = PlatformGatewayConfig {
            enabled: true,
            app_token: Some(APP_TOKEN_MARKER.to_string()),
            ..Default::default()
        };
        let view = build_slack_view(Some(&cfg));
        assert!(view.app_token_present);
        let rendered = serde_json::to_string(&view).expect("DTO must serialize to a string");
        assert!(
            !rendered.contains(APP_TOKEN_MARKER),
            "the app_token VALUE must never reach the serialized Slack DTO"
        );
    }

    // -------------------------------------------------------------------
    // Phase 49.3 Plan 03 — set_discord_enabled_impl / set_slack_enabled_impl
    // gate / round-trip (disk-backed), mirror the Telegram tests above.
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn discord_write_refuses_when_gate_closed_before_touching_config() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let cfg = seeded_config(false);
        cfg.save().expect("seed root config.yaml");
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let result = set_discord_enabled_impl(ConfigScope::Root, true).await;

        let after = std::fs::read(&config_path).expect("read config after refused write");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err());
        assert_eq!(before, after);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn slack_write_refuses_when_gate_closed_before_touching_config() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let cfg = seeded_config(false);
        cfg.save().expect("seed root config.yaml");
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let result = set_slack_enabled_impl(ConfigScope::Root, true).await;

        let after = std::fs::read(&config_path).expect("read config after refused write");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err());
        assert_eq!(before, after);
    }

    // -------------------------------------------------------------------
    // Phase 49.3 Plan 03 — validate_and_normalize_chat_edit
    // -------------------------------------------------------------------

    #[test]
    fn chat_edit_with_empty_home_channel_id_string_normalizes_to_none() {
        let payload = ChatEditPayload {
            whitelist: vec![],
            home_channel_id: Some("   ".to_string()),
            session_timeout_hours: 24,
            max_concurrent_runs: 8,
        };
        let normalized = validate_and_normalize_chat_edit(&payload).expect("must validate");
        assert_eq!(normalized.home_channel_id, None);
    }

    #[test]
    fn chat_edit_rejects_newline_in_home_channel_id() {
        let payload = ChatEditPayload {
            whitelist: vec![],
            home_channel_id: Some("chan\n1".to_string()),
            session_timeout_hours: 24,
            max_concurrent_runs: 8,
        };
        let result = validate_and_normalize_chat_edit(&payload);
        assert!(result.is_err());
        assert!(result.unwrap_err()[0].contains("newline"));
    }

    #[test]
    fn chat_edit_rejects_when_whitelist_invalid_and_names_the_problem() {
        let payload = ChatEditPayload {
            whitelist: vec!["   ".to_string()],
            home_channel_id: None,
            session_timeout_hours: 24,
            max_concurrent_runs: 8,
        };
        let result = validate_and_normalize_chat_edit(&payload);
        assert!(result.is_err());
        assert!(result.unwrap_err()[0].contains("empty or whitespace-only"));
    }

    // -------------------------------------------------------------------
    // Phase 49.3 Plan 03 — set_discord_edit_impl tempdir round trip
    // (acceptance criterion: proves whitelist+home_channel write and
    // re-read, and does NOT alter the token field).
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn discord_edit_writes_whitelist_and_home_channel_and_never_alters_the_token_field() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let mut cfg = seeded_config(true);
        let discord = PlatformGatewayConfig {
            enabled: true,
            token: Some("discord-token-should-never-move".to_string()),
            whitelist: vec!["existing-id".to_string()],
            ..Default::default()
        };
        cfg.gateway
            .platforms
            .insert(DISCORD_PLATFORM_KEY.to_string(), discord);
        cfg.save().expect("seed root config.yaml");

        let payload = ChatEditPayload {
            whitelist: vec!["111222333".to_string(), "444555666".to_string()],
            home_channel_id: Some("c-home".to_string()),
            session_timeout_hours: 48,
            max_concurrent_runs: 4,
        };
        let result = set_discord_edit_impl(ConfigScope::Root, payload).await;
        let reloaded = ironhermes_core::config::Config::load();
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        let view = result.expect("edit write must succeed when the gate is open");
        assert_eq!(
            view.whitelist,
            vec!["111222333".to_string(), "444555666".to_string()]
        );
        assert_eq!(view.home_channel_id, Some("c-home".to_string()));
        assert_eq!(view.session_timeout_hours, 48);
        assert_eq!(view.max_concurrent_runs, 4);

        let reloaded = reloaded.expect("reload saved config");
        let reloaded_discord = reloaded
            .gateway
            .platforms
            .get(DISCORD_PLATFORM_KEY)
            .expect("discord entry must survive its own write");
        assert_eq!(
            reloaded_discord.token,
            Some("discord-token-should-never-move".to_string()),
            "set_discord_edit must NEVER alter the token field"
        );
        assert_eq!(
            reloaded_discord.whitelist,
            vec!["111222333".to_string(), "444555666".to_string()],
            "the write must be visible in a fresh re-read from disk"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn slack_edit_never_alters_the_app_token_field() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let mut cfg = seeded_config(true);
        let slack = PlatformGatewayConfig {
            enabled: true,
            token: Some("xoxb-should-never-move".to_string()),
            app_token: Some("xapp-should-never-move".to_string()),
            ..Default::default()
        };
        cfg.gateway
            .platforms
            .insert(SLACK_PLATFORM_KEY.to_string(), slack);
        cfg.save().expect("seed root config.yaml");

        let payload = ChatEditPayload {
            whitelist: vec!["U9999".to_string()],
            home_channel_id: None,
            session_timeout_hours: 24,
            max_concurrent_runs: 8,
        };
        let result = set_slack_edit_impl(ConfigScope::Root, payload).await;
        let reloaded = ironhermes_core::config::Config::load();
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        result.expect("edit write must succeed when the gate is open");
        let reloaded = reloaded.expect("reload saved config");
        let reloaded_slack = reloaded
            .gateway
            .platforms
            .get(SLACK_PLATFORM_KEY)
            .expect("slack entry must survive its own write");
        assert_eq!(
            reloaded_slack.token,
            Some("xoxb-should-never-move".to_string())
        );
        assert_eq!(
            reloaded_slack.app_token,
            Some("xapp-should-never-move".to_string()),
            "set_slack_edit must NEVER alter the app_token field"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn telegram_edit_writes_whitelist_and_home_channel_and_never_alters_the_token_field() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let mut cfg = seeded_config(true);
        let telegram = PlatformGatewayConfig {
            enabled: true,
            token: Some("telegram-token-should-never-move".to_string()),
            ..Default::default()
        };
        cfg.gateway
            .platforms
            .insert(TELEGRAM_PLATFORM_KEY.to_string(), telegram);
        cfg.save().expect("seed root config.yaml");

        let payload = ChatEditPayload {
            whitelist: vec!["12345".to_string()],
            home_channel_id: Some("home-chat".to_string()),
            session_timeout_hours: 12,
            max_concurrent_runs: 2,
        };
        let result = set_telegram_edit_impl(ConfigScope::Root, payload).await;
        let reloaded = ironhermes_core::config::Config::load();
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        let view = result.expect("edit write must succeed when the gate is open");
        assert_eq!(view.whitelist, vec!["12345".to_string()]);
        assert_eq!(view.home_channel_id, Some("home-chat".to_string()));

        let reloaded = reloaded.expect("reload saved config");
        let reloaded_telegram = reloaded
            .gateway
            .platforms
            .get(TELEGRAM_PLATFORM_KEY)
            .expect("telegram entry must survive its own write");
        assert_eq!(
            reloaded_telegram.token,
            Some("telegram-token-should-never-move".to_string()),
            "set_telegram_edit must NEVER alter the token field"
        );
    }
}

//! Phase 48.2 Plan 02 (D-01/D-02/D-03/D-04/D-12/D-13/D-22): the MCP server
//! administration surface the Tools page's MCP SERVERS section drives —
//! snippet parsing (Claude-Desktop-style JSON and raw `mcp_servers:` YAML),
//! a real probe-before-commit handshake, honest per-server status, commit
//! through the atomic writer, and a non-blocking OAuth CONNECT flow.
//!
//! # Scope resolution — duplicated, not shared, from `tools_config_api`
//!
//! This plan's `files_modified` is `manager.rs` / `mcp_admin_api.rs` / `mod.rs`
//! only; `tools_config_api.rs` is Plan 03's concurrently-edited file in a
//! sibling worktree. Its `resolve_scope_target` / `check_tools_write_gate` /
//! `save_scoped` helpers are module-private (not `pub`/`pub(crate)`), so they
//! cannot be called cross-module without a visibility change that would touch
//! a file outside this plan's declared scope and collide with Plan 03's
//! parallel edits. This module therefore carries small, byte-identical
//! duplicates of those three helpers (each ~10 lines) rather than reusing the
//! shared `pub` [`crate::server::tools_config_api::ConfigScope`] type through
//! a widened-visibility import. `ConfigScope` ITSELF is reused unchanged —
//! only the private plumbing around it is duplicated.
//!
//! # Write-class actions (D-04)
//!
//! A probe causes the server host to execute an operator-supplied command or
//! make an outbound network request — it is gated exactly like a write, even
//! though it writes no file (Task 3).
//!
//! # Error strings never leak parser internals or operator secrets
//!
//! Every error this module returns is a fixed, constructed message naming
//! the server/field involved. Raw parser `Display` output (which can embed
//! the parsed value, including a secret pasted into `env`/`headers`) is
//! never forwarded — the same discipline `dotenvy::Error::LineParse`'s
//! full-line-embed leak (CR-05/CR-06) established as precedent.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::server::tools_config_api::ConfigScope;

// =============================================================================
// DTOs — shared shape on both the wasm client and the native server.
// =============================================================================

/// Which transport a server draft/config uses (D-01/D-02).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum McpTransportKind {
    Stdio,
    Http,
}

/// One server definition reduced to a browser-safe, format-agnostic shape.
/// Produced by [`parse_mcp_snippet`] (from a paste) and by [`list_mcp_servers`]
/// (from an already-configured server) — the SAME type either way, so the
/// commit path never cares which one produced it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct McpServerDraft {
    pub name: String,
    pub transport: McpTransportKind,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub url: Option<String>,
    pub headers: Vec<(String, String)>,
    pub enabled: bool,
    pub timeout: u64,
    pub connect_timeout: u64,
    pub oauth_provider: Option<String>,
    pub allowed_issuer: Option<String>,
}

/// One entry from a pasted snippet: either a successfully parsed draft, or a
/// named parse failure. Per-entry — a malformed sibling never aborts the
/// whole paste (D-01/D-02 partial-parse isolation).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct McpSnippetEntry {
    pub name: String,
    pub draft: Option<McpServerDraft>,
    pub error: Option<String>,
}

/// The full result of [`parse_mcp_snippet`] — one entry per server name found
/// in the pasted text.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct McpSnippetParse {
    pub entries: Vec<McpSnippetEntry>,
}

/// A server's honest, live-earned status (Task 3, D-03/D-12). `AuthRequired`
/// and `SpawnFailed` are kept distinguishable for every server, not just
/// newly imported ones — the manager alone cannot tell "needs OAuth" apart
/// from "actually broken" (a server that skips connecting for want of a
/// token returns success with zero tools, landing in the same bucket a real
/// handshake failure would).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum McpServerStatus {
    Connected,
    AuthRequired,
    /// Phase 48.2 Plan 09 (D-03): a real OAuth CONNECT attempt has produced
    /// an authorization URL and is waiting on the operator to complete it in
    /// their browser. Distinct from [`Self::Connecting`] — this state means
    /// the ball is in the operator's court, not the server's.
    AwaitingAuthorization { auth_url: String },
    /// Phase 48.2 Plan 09 (D-03): a CONNECT attempt is in flight — either the
    /// discovery/DCR round trip is running, or a callback has landed and the
    /// manager reload is finishing. The operator has nothing to do here; the
    /// row is honestly telling them work is happening, not asking for input.
    Connecting,
    SpawnFailed { reason: String },
    Disabled,
}

/// One row of the MCP SERVERS section (D-01/D-02/D-20/D-12).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct McpServerRow {
    pub name: String,
    pub transport: McpTransportKind,
    pub enabled: bool,
    /// `mcp__<sanitize_server_name(name)>` — the display group Plan 04 cross-links
    /// a Tools-page card group to this row, computed server-side so the browser
    /// never runs the sanitizer itself.
    pub toolset_group: String,
    pub draft: McpServerDraft,
    pub status: McpServerStatus,
}

/// One tool discovered by a probe (Task 3, D-04) — data only, never
/// registered into the live `ToolRegistry`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct McpDiscoveredTool {
    pub name: String,
    pub description: String,
}

/// The result of a one-off [`probe_mcp_server`] handshake (Task 3, D-04). An
/// OAuth server (with or without a cached token) reports `passed: true` /
/// `AuthRequired` without ever attempting a live connection — see
/// [`run_probe`]'s doc comment for why.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct McpProbeResult {
    pub passed: bool,
    pub status: McpServerStatus,
    pub tools: Vec<McpDiscoveredTool>,
    pub message: Option<String>,
}

// =============================================================================
// Server-only helpers — pure where possible so tests never need a running
// server (mirrors `tools_config_api.rs`'s test-reachability discipline).
// =============================================================================

/// D-08 sibling of `tools_config_api::resolve_scope_target` (module doc:
/// duplicated, not shared, across the worktree boundary). Resolves `scope` to
/// a fresh on-disk `Config` plus the path a save must target (`None` = root's
/// hardcoded path, `Some(path)` = a profile's `config.yaml`).
#[cfg(not(target_arch = "wasm32"))]
fn resolve_scope_target(
    scope: &ConfigScope,
) -> Result<(ironhermes_core::config::Config, Option<std::path::PathBuf>), String> {
    match scope {
        ConfigScope::Root => {
            let config = ironhermes_core::config::Config::load()
                .map_err(|e| format!("Config load failed: {e}"))?;
            Ok((config, None))
        }
        ConfigScope::Profile(name) => {
            let validated = ironhermes_core::profile::validate_profile_name(name)
                .map_err(|e| format!("invalid profile name: {e}"))?;
            let config_path =
                crate::server::profile_api::profile_dir_for(&validated).join("config.yaml");
            let config = ironhermes_core::config::Config::load_from(&config_path)
                .map_err(|e| format!("profile config load failed: {e}"))?;
            Ok((config, Some(config_path)))
        }
    }
}

/// D-10 sibling of `tools_config_api::check_tools_write_gate` (module doc):
/// fail-closed write gate reading `security.web_config_write_enabled` from a
/// FRESH ROOT `Config::load()` regardless of the scope being edited. A probe
/// is a write-class action (D-04) and checks this gate too.
#[cfg(not(target_arch = "wasm32"))]
fn check_mcp_write_gate() -> Result<(), String> {
    let root_config = ironhermes_core::config::Config::load()
        .map_err(|e| format!("Config load failed: {e}"))?;
    if !root_config.security.web_config_write_enabled {
        return Err("Config writes are disabled".to_string());
    }
    Ok(())
}

/// D-13 sibling of `tools_config_api::save_scoped` (module doc): atomic write
/// — `Config::save()` for root, `Config::save_to(path)` for a profile.
#[cfg(not(target_arch = "wasm32"))]
fn save_scoped(
    config: &ironhermes_core::config::Config,
    target: &Option<std::path::PathBuf>,
) -> Result<(), String> {
    match target {
        None => config
            .save()
            .map_err(|e| format!("Config save failed: {e}")),
        Some(path) => config
            .save_to(path)
            .map_err(|e| format!("Config save failed: {e}")),
    }
}

/// Either half of the JSON/YAML dichotomy for one snippet entry's raw value —
/// carried through so the SAME `McpServerConfig` deserialization codepath is
/// used regardless of which format the paste was.
#[cfg(not(target_arch = "wasm32"))]
enum RawEntryValue {
    Json(serde_json::Value),
    Yaml(serde_yaml::Value),
}

/// Split a pasted snippet into `(server_name, raw_value)` pairs. Tries JSON
/// first (Claude-Desktop-style `{"mcpServers": {...}}` or a bare object of
/// server-name keys); on JSON syntax failure, tries YAML (a top-level
/// `mcp_servers:` mapping or a bare mapping of server-name keys). Returns a
/// single top-level `Err` only when the text is neither.
#[cfg(not(target_arch = "wasm32"))]
fn extract_snippet_entries(text: &str) -> Result<Vec<(String, RawEntryValue)>, String> {
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(text) {
        let entries_map = match map.get("mcpServers") {
            Some(serde_json::Value::Object(inner)) => inner.clone(),
            _ => map,
        };
        return Ok(entries_map
            .into_iter()
            .map(|(name, value)| (name, RawEntryValue::Json(value)))
            .collect());
    }

    if let Ok(serde_yaml::Value::Mapping(map)) = serde_yaml::from_str::<serde_yaml::Value>(text) {
        let entries_map = match map.get("mcp_servers") {
            Some(serde_yaml::Value::Mapping(inner)) => inner.clone(),
            _ => map,
        };
        let mut out = Vec::new();
        for (key, value) in entries_map {
            let name = key
                .as_str()
                .ok_or_else(|| "snippet contains a non-string server name key".to_string())?
                .to_string();
            out.push((name, RawEntryValue::Yaml(value)));
        }
        return Ok(out);
    }

    Err("snippet is neither valid JSON nor valid YAML".to_string())
}

/// Reduce one already-format-agnostic `McpServerConfig` into a browser-safe
/// draft, choosing the transport from which of `command`/`url` is set.
#[cfg(not(target_arch = "wasm32"))]
fn config_to_draft(
    name: &str,
    cfg: ironhermes_mcp::McpServerConfig,
) -> Result<McpServerDraft, String> {
    let transport = match (cfg.command.is_some(), cfg.url.is_some()) {
        (true, true) => {
            return Err(format!(
                "entry '{name}' specifies both command and url; only one transport is allowed"
            ));
        }
        (true, false) => McpTransportKind::Stdio,
        (false, true) => McpTransportKind::Http,
        (false, false) => {
            return Err(format!(
                "entry '{name}' is missing required field: command or url"
            ));
        }
    };
    Ok(McpServerDraft {
        name: name.to_string(),
        transport,
        command: cfg.command,
        args: cfg.args,
        env: cfg.env.into_iter().collect(),
        url: cfg.url,
        headers: cfg.headers.into_iter().collect(),
        enabled: cfg.enabled,
        timeout: cfg.timeout,
        connect_timeout: cfg.connect_timeout,
        oauth_provider: cfg.oauth_provider,
        allowed_issuer: cfg.allowed_issuer,
    })
}

/// Parse one snippet entry into a draft, or a named per-entry error. Never
/// panics, never embeds the raw parser `Display` (which can echo back a
/// secret value in `env`/`headers`) or the raw snippet text in the returned
/// message — only fixed, constructed strings naming the server/field.
#[cfg(not(target_arch = "wasm32"))]
fn parse_one_entry(name: &str, raw: RawEntryValue) -> Result<McpServerDraft, String> {
    if ironhermes_mcp::sanitize_server_name(name) != name {
        return Err(format!(
            "server name '{name}' contains characters outside [A-Za-z0-9_]; \
             rename it before importing"
        ));
    }

    let cfg: ironhermes_mcp::McpServerConfig = match raw {
        RawEntryValue::Json(value) => serde_json::from_value(value).map_err(|_| {
            format!("entry '{name}' could not be parsed as a valid MCP server definition")
        })?,
        RawEntryValue::Yaml(value) => serde_yaml::from_value(value).map_err(|_| {
            format!("entry '{name}' could not be parsed as a valid MCP server definition")
        })?,
    };

    config_to_draft(name, cfg)
}

/// Pure core of [`parse_mcp_snippet`] — no disk I/O, no global state, directly
/// unit-testable with hand-built snippet text.
#[cfg(not(target_arch = "wasm32"))]
fn parse_snippet_text(text: &str) -> Result<McpSnippetParse, String> {
    let raw_entries = extract_snippet_entries(text)?;
    let entries = raw_entries
        .into_iter()
        .map(|(name, raw)| match parse_one_entry(&name, raw) {
            Ok(draft) => McpSnippetEntry {
                name,
                draft: Some(draft),
                error: None,
            },
            Err(error) => McpSnippetEntry {
                name,
                draft: None,
                error: Some(error),
            },
        })
        .collect();
    Ok(McpSnippetParse { entries })
}

/// Map a browser-supplied draft into the `ironhermes-mcp` config shape this
/// crate writes to `config.yaml` (D-01/D-02). Vec-of-pairs fields collapse
/// into the config's `HashMap` fields; unset options stay `None`/default.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn draft_to_server_config(draft: &McpServerDraft) -> ironhermes_mcp::McpServerConfig {
    ironhermes_mcp::McpServerConfig {
        command: draft.command.clone(),
        args: draft.args.clone(),
        env: draft.env.iter().cloned().collect(),
        url: draft.url.clone(),
        headers: draft.headers.iter().cloned().collect(),
        timeout: draft.timeout,
        connect_timeout: draft.connect_timeout,
        enabled: draft.enabled,
        enabled_tools: None,
        auth: None,
        oauth_provider: draft.oauth_provider.clone(),
        allowed_issuer: draft.allowed_issuer.clone(),
        sampling: None,
    }
}

/// Parse a raw `Config.mcp_servers` value (`serde_yaml::Value`) into a typed
/// `McpServerConfig` — the read-side twin of [`write_server_into`]. Errors
/// are a fixed message; the raw `serde_yaml::Error` `Display` is never
/// forwarded (it can echo the parsed value).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn server_config_from_value(
    value: &serde_yaml::Value,
) -> Result<ironhermes_mcp::McpServerConfig, String> {
    serde_yaml::from_value(value.clone())
        .map_err(|_| "stored server definition could not be parsed".to_string())
}

/// D-13: the ONLY correct write path into `Config.mcp_servers` — that field
/// is a raw `serde_yaml::Value` map by design (the core crate cannot depend
/// on `ironhermes-mcp`), so every committed server goes through
/// `serde_yaml::to_value` here, never a hand-built YAML fragment.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn write_server_into(
    config: &mut ironhermes_core::config::Config,
    name: &str,
    cfg: &ironhermes_mcp::McpServerConfig,
) -> Result<(), String> {
    let value = serde_yaml::to_value(cfg)
        .map_err(|_| format!("server '{name}' could not be serialized for config write"))?;
    config.mcp_servers.insert(name.to_string(), value);
    Ok(())
}

/// Pure classification (Task 3, D-03/D-12; warm-but-revoked follow-up fix):
/// disabled first, then the OAuth-needs-(re)authorization check, then
/// connected, then spawn-failed — in that exact precedence. The OAuth check
/// MUST run before connected/failed: a server that skips connecting for want
/// of a usable token returns success with zero tools and would otherwise
/// land in the same "failed" bucket a real handshake failure produces.
///
/// The OAuth check has two ways to fire, not one: `!has_token` (the cold
/// case — nobody has ever authorized) OR the server is not connected AND
/// `failure_reason` carries one of `is_oauth_reauthorization_required`'s
/// fixed markers (the warm-but-revoked case — a token IS stored, but the
/// live retry loop's most recent error says the authorization server will
/// not honor it, e.g. a refresh returning `invalid_grant`). `has_token`
/// alone is presence-only (`McpManager::has_oauth_token`) and cannot tell a
/// live credential from a dead one; the failure-reason check is what closes
/// that gap. A genuine spawn/transport failure on an OAuth server whose
/// token is otherwise fine (has_token AND no auth-caused failure reason)
/// still falls through to `SpawnFailed` below — this check does not fire
/// merely because the server is not connected.
///
/// Deviation from the plan's literal signature: `name: &str` is added as the
/// first parameter. Without it, `connected: &HashSet<String>` cannot answer
/// "is THIS server connected" — the set alone has no way to select the row
/// being classified. This is the minimal fix that makes the documented
/// `<behavior>` cases (which each classify ONE named server) implementable.
#[cfg(not(target_arch = "wasm32"))]
fn classify_server_status(
    name: &str,
    cfg: &ironhermes_mcp::McpServerConfig,
    connected: &std::collections::HashSet<String>,
    failure_reason: Option<&str>,
    has_token: bool,
) -> McpServerStatus {
    if !cfg.enabled {
        return McpServerStatus::Disabled;
    }
    if cfg.oauth_provider.is_some() {
        let not_connected = !connected.contains(name);
        let auth_caused_failure = not_connected
            && failure_reason
                .map(ironhermes_mcp::security::is_oauth_reauthorization_required)
                .unwrap_or(false);
        if !has_token || auth_caused_failure {
            return McpServerStatus::AuthRequired;
        }
    }
    if connected.contains(name) {
        return McpServerStatus::Connected;
    }
    McpServerStatus::SpawnFailed {
        reason: failure_reason.unwrap_or("not connected").to_string(),
    }
}

/// Read the fresh root `Config.mcp_oauth.issuer_allowlist` — the same global
/// additive list `McpManager` is built from at startup (`with_global_issuer_allowlist`),
/// re-read here directly from `Config` since the manager exposes no getter for
/// its copy. Used only for issuer validation, never to reconstruct an `AuthStore`.
#[cfg(not(target_arch = "wasm32"))]
fn root_global_issuer_allowlist() -> Vec<String> {
    ironhermes_core::config::Config::load()
        .map(|c| c.mcp_oauth.issuer_allowlist)
        .unwrap_or_default()
}

// =============================================================================
// Phase 48.2 Plan 09 (D-03/D-09/T-48.2-09-02): the web-completable MCP OAuth
// authorization start — redirect-origin resolution, the browser affordance
// state machine, and the web CONNECT integration seam. This module never
// binds a listener, never launches a browser, and never blocks on network
// I/O inline — every real handshake step happens through
// `ironhermes_mcp::McpManager::begin_oauth`/`complete_oauth`/`cancel_oauth`.
// =============================================================================

/// The fixed MCP OAuth web callback path — the ONE spelling shared by this
/// module's redirect-URI construction (Task 1) and the route mounted in
/// `main.rs`/`login_page.rs::test_router` (Task 2/3, 48.2-09).
pub(crate) const MCP_OAUTH_CALLBACK_PATH: &str = "/oauth/mcp/callback";

/// Read the fresh root `Config.mcp_oauth.web_redirect_base_url` — the
/// operator-pinned public origin (D-13, 48.2-08), sibling to
/// [`root_global_issuer_allowlist`]'s fresh-`Config::load()` pattern.
#[cfg(not(target_arch = "wasm32"))]
fn root_web_redirect_base_url() -> Option<String> {
    ironhermes_core::config::Config::load()
        .ok()
        .and_then(|c| c.mcp_oauth.web_redirect_base_url)
}

/// Pure precedence + validation: the operator-configured redirect base wins
/// whenever it is present and non-blank; otherwise the browser-supplied
/// origin is used. Either way, the chosen candidate must pass
/// [`ironhermes_mcp::security::validate_web_redirect_base`] — its rejection
/// (a fixed message that never echoes the input) is returned as-is.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_redirect_base(configured: Option<&str>, browser_origin: &str) -> Result<String, String> {
    let candidate = match configured {
        Some(v) if !v.trim().is_empty() => v,
        _ => browser_origin,
    };
    ironhermes_mcp::security::validate_web_redirect_base(candidate)
}

/// Resolve `(connected_names, has_token, failure_reason)` for one server
/// against the LIVE manager, if one is installed (test-reachability
/// discipline — `None`/empty when no `AppState` is installed, mirroring
/// `tools_config_api::live_catalog_rows`). `has_token` is answered ONLY
/// through [`ironhermes_mcp::McpManager::has_oauth_token`] — the sanctioned,
/// presence-only path (REVIEWS finding 2); `failure_reason` is answered ONLY
/// through [`ironhermes_mcp::McpManager::last_failure_reason`] — the sibling
/// presence/string-only path the warm-but-revoked follow-up fix added. This
/// module never names `AuthStore` and never constructs a second store.
#[cfg(not(target_arch = "wasm32"))]
async fn classify_status_live(
    name: &str,
    cfg: &ironhermes_mcp::McpServerConfig,
) -> McpServerStatus {
    let manager = crate::server::state::try_global_app_state()
        .and_then(|state| state.runtime.mcp_manager().cloned());
    let (connected, has_token, failure_reason) = match &manager {
        Some(manager) => {
            let connected: std::collections::HashSet<String> =
                manager.connected_server_names().into_iter().collect();
            let has_token = match cfg.oauth_provider.as_deref() {
                Some(ns) => manager.has_oauth_token(ns).await,
                None => false,
            };
            // Warm-but-revoked follow-up fix: this used to be hardcoded
            // `None` with a comment claiming no cheap live lookup existed.
            // McpManager::last_failure_reason is that lookup now — populated
            // continuously by the retry loop, not only at reload time.
            let failure_reason = manager.last_failure_reason(name).await;
            (connected, has_token, failure_reason)
        }
        None => (std::collections::HashSet::new(), false, None),
    };
    classify_server_status(name, cfg, &connected, failure_reason.as_deref(), has_token)
}

/// Build one [`McpServerRow`] for `name` from a freshly resolved `config`
/// (Task 3). Returns `Err` only when `name` is absent from `config.mcp_servers`
/// or its stored value fails to parse.
#[cfg(not(target_arch = "wasm32"))]
async fn row_for(
    name: &str,
    config: &ironhermes_core::config::Config,
) -> Result<McpServerRow, String> {
    let raw = config
        .mcp_servers
        .get(name)
        .ok_or_else(|| format!("server '{name}' not found"))?;
    let cfg = server_config_from_value(raw)?;
    let transport = if cfg.command.is_some() {
        McpTransportKind::Stdio
    } else {
        McpTransportKind::Http
    };
    let enabled = cfg.enabled;
    let draft = config_to_draft(name, cfg.clone()).unwrap_or(McpServerDraft {
        name: name.to_string(),
        transport: transport.clone(),
        command: None,
        args: Vec::new(),
        env: Vec::new(),
        url: None,
        headers: Vec::new(),
        enabled,
        timeout: cfg.timeout,
        connect_timeout: cfg.connect_timeout,
        oauth_provider: cfg.oauth_provider.clone(),
        allowed_issuer: cfg.allowed_issuer.clone(),
    });
    let status = classify_status_live(name, &cfg).await;
    Ok(McpServerRow {
        name: name.to_string(),
        transport,
        enabled,
        toolset_group: format!("mcp__{}", ironhermes_mcp::sanitize_server_name(name)),
        draft,
        status,
    })
}

/// D-12 live-apply helper (Task 3). For `ConfigScope::Root`, deserializes
/// every `config.mcp_servers` entry and calls the manager's bulk
/// reload-and-report primitive with the full map; for `ConfigScope::Profile`,
/// returns `None` — profile agents (kanban workers, bot-mode subprocesses)
/// read their config at process launch, so there is nothing in-process to
/// refresh.
///
/// **The manager exposes only bulk reload primitives** — both shut every
/// server down before restarting, so one server's change momentarily
/// disconnects the others (`48.2-RESEARCH.md` Assumption A2, Pitfall 2). No
/// per-server start/stop API is added to `ironhermes-mcp`; Plan 04 surfaces
/// this blast radius in the UI copy.
///
/// Deviation from the plan's literal signature: `scope: &ConfigScope` is
/// added as the first parameter — the doc comment's own Root/Profile branch
/// requires knowing the scope, which the listed signature omits.
#[cfg(not(target_arch = "wasm32"))]
async fn reload_mcp_and_report(
    scope: &ConfigScope,
    config: &ironhermes_core::config::Config,
) -> Option<ironhermes_mcp::StartResult> {
    if !matches!(scope, ConfigScope::Root) {
        return None;
    }
    let manager = crate::server::state::try_global_app_state()
        .and_then(|state| state.runtime.mcp_manager().cloned())?;
    let new_configs: std::collections::HashMap<String, ironhermes_mcp::McpServerConfig> = config
        .mcp_servers
        .iter()
        .filter_map(|(name, value)| {
            server_config_from_value(value)
                .ok()
                .map(|cfg| (name.clone(), cfg))
        })
        .collect();
    Some(manager.reload_and_report(new_configs).await)
}

/// Attempt a one-off stdio or plain-HTTP connection and list its tools.
/// Returns `Err(())` deliberately opaque — the caller constructs every
/// user-facing message from fixed text naming the server, never this
/// function's internal error detail (which could echo the spawn environment,
/// full command line, or a header value).
///
/// No `ToolRegistry` is touched anywhere in this function — the discovered
/// tools are returned as plain data (REVIEWS finding 3 / T-48.2-02-10): a
/// probe that needed a registry-unregister call on cleanup would mean it had
/// already leaked into the live registry, which is the bug this discipline
/// prevents.
/// The connection type (`rmcp::service::RunningService<...>`) never crosses
/// this function's signature — `rmcp` is not a direct dependency of this
/// crate (only of `ironhermes-mcp`, which this plan's `files_modified` does
/// not extend to `Cargo.toml` changes for). `client` is dropped explicitly
/// before returning on the success path, inside this function, so no caller
/// ever needs to name the type either.
#[cfg(not(target_arch = "wasm32"))]
async fn connect_and_list_non_oauth(
    cfg: &ironhermes_mcp::McpServerConfig,
) -> Result<Vec<McpDiscoveredTool>, ()> {
    let (client, _child) = if cfg.command.is_some() {
        ironhermes_mcp::transport::connect_stdio(cfg)
            .await
            .map_err(|_| ())?
    } else if cfg.url.is_some() {
        ironhermes_mcp::transport::connect_http(cfg)
            .await
            .map_err(|_| ())?
    } else {
        return Err(());
    };
    let mcp_tools = client.list_all_tools().await.map_err(|_| ())?;
    let tools = mcp_tools
        .iter()
        .map(|t| McpDiscoveredTool {
            name: t.name.to_string(),
            description: t.description.as_deref().unwrap_or_default().to_string(),
        })
        .collect();
    // Explicit teardown on the success path — kill_on_drop (stdio) or a
    // plain HTTP client teardown either way, mirroring `McpManager::shutdown_all`'s
    // own reliance on drop-triggered cleanup for the Option B fallback.
    drop(client);
    Ok(tools)
}

/// Pure(-ish) core of [`probe_mcp_server`] (Task 3, D-04/T-48.2-02-01).
///
/// # OAuth servers never connect from this module
///
/// `McpManager` exposes ONLY a presence-only `has_oauth_token(namespace) -> bool`
/// accessor (Task 2, REVIEWS finding 2) — this module has no way to obtain
/// `Arc<AuthStore>` and must not construct a second store. An OAuth-configured
/// draft (with or without a cached token) therefore never attempts a live
/// connection here; it still runs the SAME issuer resolution and validation
/// `ironhermes-mcp`'s own OAuth connect path runs
/// (`security::resolve_allowed_issuers` / `security::validate_prm_issuer`)
/// before reporting `AuthRequired`, so a disallowed issuer is rejected even
/// at probe time. Real verification for an OAuth server happens only through
/// the Task 4 CONNECT flow, whose outcome is read back from the live manager.
///
/// # Cleanup on every exit path
///
/// The non-OAuth connect attempt is wrapped in `tokio::time::timeout`,
/// bounded by the draft's `connect_timeout`. Async cancellation drops every
/// value owned by the timed-out future — including a `client`/transport that
/// was mid-handshake — which for a stdio child mirrors `McpManager::shutdown_all`'s
/// own reliance on `kill_on_drop(true)` (set inside `connect_stdio`'s configure
/// closure): the OS process receives SIGKILL as part of that drop, with no
/// separately-tracked child handle required. On the success path, `client` is
/// dropped explicitly before returning.
#[cfg(not(target_arch = "wasm32"))]
async fn run_probe(name: &str, cfg: &ironhermes_mcp::McpServerConfig) -> McpProbeResult {
    if cfg.oauth_provider.is_some() {
        let global_allowlist = root_global_issuer_allowlist();
        let allowed =
            ironhermes_mcp::security::resolve_allowed_issuers(cfg.allowed_issuer.as_deref(), &global_allowlist);
        if let Some(url) = cfg.url.as_deref() {
            if let Err(_e) = ironhermes_mcp::security::validate_prm_issuer(url, &allowed) {
                return McpProbeResult {
                    passed: false,
                    status: McpServerStatus::SpawnFailed {
                        reason: format!("issuer validation failed for '{name}'"),
                    },
                    tools: Vec::new(),
                    message: Some(format!(
                        "probe of '{name}' rejected: the server's issuer is not in the allowed set"
                    )),
                };
            }
        }
        return McpProbeResult {
            passed: true,
            status: McpServerStatus::AuthRequired,
            tools: Vec::new(),
            message: None,
        };
    }

    let connect_timeout = std::time::Duration::from_secs(cfg.connect_timeout.max(1));
    match tokio::time::timeout(connect_timeout, connect_and_list_non_oauth(cfg)).await {
        Ok(Ok(tools)) => McpProbeResult {
            passed: true,
            status: McpServerStatus::Connected,
            tools,
            message: None,
        },
        Ok(Err(())) => McpProbeResult {
            passed: false,
            status: McpServerStatus::SpawnFailed {
                reason: format!("probe of '{name}' failed to connect"),
            },
            tools: Vec::new(),
            message: Some(format!(
                "probe of '{name}' failed: could not establish an MCP connection"
            )),
        },
        Err(_elapsed) => McpProbeResult {
            passed: false,
            status: McpServerStatus::SpawnFailed {
                reason: format!("probe of '{name}' timed out"),
            },
            tools: Vec::new(),
            message: Some(format!("probe of '{name}' timed out")),
        },
    }
}

/// One OAuth CONNECT attempt's state (Task 4/48.2-09, D-03/RESEARCH Pitfall 3).
///
/// `InFlight` (discovery/DCR running), `AwaitingAuthorization` (a real
/// authorization URL exists and the operator must act on it in their
/// browser), `Finalizing` (a callback landed and the manager reload is
/// running), `Failed`, and `Succeeded` are five distinguishable answers —
/// D-03's honest-status contract forbids collapsing any of them into another.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug)]
enum OAuthAttemptState {
    InFlight { started_at: std::time::Instant },
    /// Phase 48.2 Plan 09: a real authorization URL was produced and is
    /// waiting on the operator. `auth_url` is cloned out into
    /// [`McpServerStatus::AwaitingAuthorization`] whenever this state is
    /// read back.
    AwaitingAuthorization {
        auth_url: String,
        started_at: std::time::Instant,
    },
    /// Phase 48.2 Plan 09: an authorization callback landed and
    /// `reload_mcp_and_report` is running in the background. Maps to
    /// [`McpServerStatus::Connecting`] — CONNECTED is earned only once the
    /// reload finishes and records `Succeeded` (D-12).
    Finalizing { started_at: std::time::Instant },
    Succeeded,
    Failed { reason: String },
}

/// Process-lifetime map of in-flight/finished OAuth CONNECT attempts, keyed
/// by `"{scope:?}::{name}"` (`ConfigScope` derives `Debug`/`PartialEq` but
/// not `Eq`/`Hash` — out of scope to add here, so a formatted string key is
/// used instead of a tuple key).
#[cfg(not(target_arch = "wasm32"))]
static OAUTH_ATTEMPTS: std::sync::OnceLock<
    tokio::sync::Mutex<std::collections::HashMap<String, OAuthAttemptState>>,
> = std::sync::OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
fn oauth_attempts()
-> &'static tokio::sync::Mutex<std::collections::HashMap<String, OAuthAttemptState>> {
    OAUTH_ATTEMPTS.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()))
}

#[cfg(not(target_arch = "wasm32"))]
fn oauth_attempt_key(scope: &ConfigScope, name: &str) -> String {
    format!("{scope:?}::{name}")
}

/// Phase 48.2 Plan 09 (D-03/T-48.2-09-01): process-lifetime index from an
/// OAuth `state` value to the `(ConfigScope, server name)` pair that started
/// it. This is the ONLY way the stateless web callback route (Task 2) learns
/// which attempt, and which config scope, it is finishing — the callback
/// carries no session, no scope, nothing but `code`/`state`/`iss` (or
/// `error`). Stores the real `ConfigScope` value rather than re-parsing it
/// out of `oauth_attempt_key`'s formatted `{scope:?}` spelling, which is a
/// `Debug` rendering and not a parseable format.
#[cfg(not(target_arch = "wasm32"))]
static OAUTH_STATE_INDEX: std::sync::OnceLock<
    tokio::sync::Mutex<std::collections::HashMap<String, (ConfigScope, String)>>,
> = std::sync::OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn oauth_state_index()
-> &'static tokio::sync::Mutex<std::collections::HashMap<String, (ConfigScope, String)>> {
    OAUTH_STATE_INDEX.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()))
}

/// UAT round 2 (Task 5 fix A, 48.2-09 continuation): the authorization-start
/// path — AS discovery plus dynamic client registration inside
/// `McpManager::begin_oauth` — has no bound of its own; it is a live network
/// round trip that can hang indefinitely (an idle, half-open TCP connection
/// to the authorization server was captured live during the reproduction).
/// A hung authorization start must never leave the row's poll stuck showing
/// a spinner forever, so it is wrapped in this fixed timeout.
///
/// 30 seconds is chosen because it is: comfortably longer than any
/// well-behaved discovery + DCR round trip (sub-second to a few seconds in
/// practice); short enough that an operator staring at a spinner gets an
/// honest answer inside the patience window a UI interaction implies; and
/// far short of both 48.2-08's 300s pending-authorization TTL and this
/// module's own 360s `STALE_AFTER` self-heal, so a timeout firing here is
/// never confused with — or masked by — either of those slower ceilings.
#[cfg(not(target_arch = "wasm32"))]
const OAUTH_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Bounds an authorization-start future with an explicit `duration`. Split
/// out from [`perform_oauth_connect`], and parameterized on the duration
/// rather than reading [`OAUTH_CONNECT_TIMEOUT`] directly, so the timeout
/// itself — not just `perform_oauth_connect`'s missing-manager fast-fail
/// branch — is directly unit-testable: a test can drive this with a
/// millisecond-scale duration and a future that never resolves (standing in
/// for a hung `begin_oauth` network call) and observe a real firing of the
/// timeout in well under a second, with no virtual-clock machinery needed.
#[cfg(not(target_arch = "wasm32"))]
async fn await_with_oauth_timeout_after<F>(
    duration: std::time::Duration,
    fut: F,
    timeout_message: impl FnOnce() -> String,
) -> Result<(String, String), String>
where
    F: std::future::Future<Output = Result<(String, String), String>>,
{
    match tokio::time::timeout(duration, fut).await {
        Ok(inner) => inner,
        Err(_elapsed) => Err(timeout_message()),
    }
}

/// Production entry point: bounds an authorization-start future with the
/// real [`OAUTH_CONNECT_TIMEOUT`]. See [`await_with_oauth_timeout_after`]
/// for why the duration is a parameter rather than baked in here.
#[cfg(not(target_arch = "wasm32"))]
async fn await_with_oauth_timeout<F>(fut: F, timeout_message: impl FnOnce() -> String) -> Result<(String, String), String>
where
    F: std::future::Future<Output = Result<(String, String), String>>,
{
    await_with_oauth_timeout_after(OAUTH_CONNECT_TIMEOUT, fut, timeout_message).await
}

/// The OAuth-connect integration seam (Task 4/48.2-09 Task 1).
///
/// Reaches the live `McpManager` through `try_global_app_state()` and calls
/// [`ironhermes_mcp::McpManager::begin_oauth`] with the resolved
/// `redirect_uri`, returning `(auth_url, state)` on success. This function
/// never names or constructs `AuthStore` — the manager owns the entire OAuth
/// session lifecycle behind its string-only `begin_oauth`/`complete_oauth`/
/// `cancel_oauth` surface (T-48.2-02-08 continues to hold; 48.2-08 gave the
/// manager the pair this seam now calls).
///
/// Errors with fixed text when no manager is installed in this process (the
/// unit-test posture, and any deployment that never wired an `AgentRuntime`),
/// and with fixed text when [`OAUTH_CONNECT_TIMEOUT`] elapses before
/// `begin_oauth` returns (UAT round 2 fix A — see that constant's doc for
/// why 30s). The manager's own `begin_oauth` already sanitizes every error
/// before returning it, so nothing from `config`, `redirect_uri`, or the
/// manager's internal error detail is ever echoed here.
#[cfg(not(target_arch = "wasm32"))]
async fn perform_oauth_connect(
    name: &str,
    cfg: &ironhermes_mcp::McpServerConfig,
    redirect_uri: &str,
) -> Result<(String, String), String> {
    let manager = crate::server::state::try_global_app_state()
        .and_then(|state| state.runtime.mcp_manager().cloned())
        .ok_or_else(|| "no MCP manager installed in this process".to_string())?;
    await_with_oauth_timeout(
        async { manager.begin_oauth(name, cfg, redirect_uri).await.map(|start| (start.auth_url, start.state)) },
        || {
            format!(
                "authorization start for '{name}' timed out after {}s waiting on the authorization \
                 server (discovery/dynamic client registration did not respond)",
                OAUTH_CONNECT_TIMEOUT.as_secs()
            )
        },
    )
    .await
}

/// UAT round 2 (Task 5 fix A): the ONE place an OAuth CONNECT attempt's
/// outcome is recorded into [`OAUTH_ATTEMPTS`] (and, on success,
/// [`OAUTH_STATE_INDEX`]). Every exit from the background connect task —
/// ordinary success, an ordinary error, the [`OAUTH_CONNECT_TIMEOUT`] firing,
/// or the watchdog in [`oauth_connect_watchdog`] observing a panicked/
/// cancelled task — funnels through this one function, so a stuck
/// `InFlight`/`Connecting` pill is not reachable by any route out of the
/// spawn body.
#[cfg(not(target_arch = "wasm32"))]
async fn record_oauth_connect_outcome(scope: ConfigScope, name: String, outcome: Result<(String, String), String>) {
    let key = oauth_attempt_key(&scope, &name);
    match outcome {
        Ok((auth_url, state)) => {
            oauth_attempts().lock().await.insert(
                key,
                OAuthAttemptState::AwaitingAuthorization {
                    auth_url,
                    started_at: std::time::Instant::now(),
                },
            );
            oauth_state_index().lock().await.insert(state, (scope, name));
        }
        Err(reason) => {
            oauth_attempts().lock().await.insert(key, OAuthAttemptState::Failed { reason });
        }
    }
}

/// UAT round 2 (Task 5 fix A): wraps the authorization-start future in a
/// nested `tokio::spawn` and awaits its `JoinHandle` here, so a panic inside
/// `fut` — which tokio isolates to the inner task and would otherwise never
/// be observed by anything, leaving `OAUTH_ATTEMPTS` stuck at `InFlight`
/// forever — surfaces as an ordinary `JoinError` instead. Every exit
/// (success, ordinary error, timeout inside `fut`, panic, or cancellation)
/// is recorded via [`record_oauth_connect_outcome`], which is the only
/// writer of a terminal state for this attempt.
#[cfg(not(target_arch = "wasm32"))]
async fn oauth_connect_watchdog(
    scope: ConfigScope,
    name: String,
    fut: impl std::future::Future<Output = Result<(String, String), String>> + Send + 'static,
) {
    let handle = tokio::spawn(fut);
    let outcome = match handle.await {
        Ok(result) => result,
        Err(join_err) => Err(format!(
            "OAuth authorization-start task ended unexpectedly ({}); the attempt was marked failed \
             rather than left waiting",
            if join_err.is_panic() { "it panicked" } else { "it was cancelled" }
        )),
    };
    record_oauth_connect_outcome(scope, name, outcome).await;
}

/// Phase 48.2 Plan 09 Task 2: the ONE public completion entry the web
/// callback route (`mcp_oauth_callback_route.rs`) calls when the
/// authorization server's redirect carries a `code`. NOT a `#[server]` fn —
/// called directly from the raw axum route handler, which is itself outside
/// the auth wall (T-48.2-09-01) and therefore never routed through Dioxus's
/// server-fn dispatch.
///
/// Extracts `state` from `callback_url`; looks it up in [`OAUTH_STATE_INDEX`]
/// and errors with fixed text when absent — an unknown, expired, or
/// already-used state must never reach a token exchange. Obtains the live
/// manager and errors with fixed text when none is installed. Calls
/// [`ironhermes_mcp::McpManager::complete_oauth`]. On success, records
/// `Finalizing`, removes the index entry, and spawns a task that runs
/// [`reload_mcp_and_report`] for the recorded scope and only THEN records
/// `Succeeded` — so the poll sees `Connecting` while the reload runs and
/// `Connected` only once the manager has actually reconnected (D-12's
/// "earned, never assumed" contract). This function does not await that
/// reload — the caller (the route handler) returns the browser its page
/// immediately. On the error branch, records `Failed { reason }`, removes
/// the index entry, and returns the sanitized error `complete_oauth` already
/// produced.
/// The fixed refusal text for an unknown, already-used, or expired OAuth
/// `state`. Shared verbatim by [`complete_oauth_from_callback`]'s error
/// return and `mcp_oauth_callback_route.rs`'s 404-vs-400 status decision so
/// the two spellings can never drift apart — the route matches on this exact
/// string to decide "unknown state" (404) from every other failure (400).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const UNKNOWN_OAUTH_STATE_MESSAGE: &str =
    "unknown, already-used, or expired authorization state";

#[cfg(not(target_arch = "wasm32"))]
pub async fn complete_oauth_from_callback(callback_url: &str) -> Result<(), String> {
    let state = ironhermes_mcp::transport::oauth_state_from_url(callback_url)
        .map_err(|_| "callback URL is missing a valid state parameter".to_string())?;

    let (scope, name) = {
        let index = oauth_state_index().lock().await;
        index.get(&state).cloned()
    }
    .ok_or_else(|| UNKNOWN_OAUTH_STATE_MESSAGE.to_string())?;

    let manager = crate::server::state::try_global_app_state()
        .and_then(|state| state.runtime.mcp_manager().cloned())
        .ok_or_else(|| "no MCP manager installed in this process".to_string())?;

    let key = oauth_attempt_key(&scope, &name);
    let result = manager.complete_oauth(callback_url).await;
    oauth_state_index().lock().await.remove(&state);

    match result {
        Ok(_server_name) => {
            oauth_attempts().lock().await.insert(
                key.clone(),
                OAuthAttemptState::Finalizing {
                    started_at: std::time::Instant::now(),
                },
            );
            tokio::spawn(async move {
                if let Ok((reloaded_config, _target)) = resolve_scope_target(&scope) {
                    reload_mcp_and_report(&scope, &reloaded_config).await;
                }
                oauth_attempts()
                    .lock()
                    .await
                    .insert(key, OAuthAttemptState::Succeeded);
            });
            Ok(())
        }
        Err(reason) => {
            oauth_attempts().lock().await.insert(
                key,
                OAuthAttemptState::Failed {
                    reason: reason.clone(),
                },
            );
            Err(reason)
        }
    }
}

/// Phase 48.2 Plan 09 Task 2: the completion entry's sibling for a denied or
/// failed authorization — called when the authorization server's redirect
/// carries an `error` parameter instead of a `code`. Removes the index
/// entry, records `Failed` with a fixed denial reason on the attempt key,
/// and calls [`ironhermes_mcp::McpManager::cancel_oauth`] so the parked
/// session is dropped rather than left to age out on the manager's own
/// 300s TTL. A `state` that is absent, unknown, or already consumed is a
/// silent no-op (mirrors `McpManager::cancel_oauth`'s own no-op-on-unknown
/// behavior) — never an error, since an operator re-visiting a stale denial
/// page has nothing further to abandon.
#[cfg(not(target_arch = "wasm32"))]
pub async fn abandon_oauth_from_callback(state: &str) {
    let entry = { oauth_state_index().lock().await.remove(state) };
    if let Some((scope, name)) = entry {
        let key = oauth_attempt_key(&scope, &name);
        oauth_attempts().lock().await.insert(
            key,
            OAuthAttemptState::Failed {
                reason: "authorization denied at the authorization server".to_string(),
            },
        );
    }
    if let Some(manager) =
        crate::server::state::try_global_app_state().and_then(|s| s.runtime.mcp_manager().cloned())
    {
        manager.cancel_oauth(state).await;
    }
}

// =============================================================================
// #[server] fns — thin wrappers over the impl fns above.
// =============================================================================

/// Parse a Claude-Desktop-style JSON block or a raw `mcp_servers:` YAML
/// fragment into per-entry drafts (D-01/D-02). Per-entry failures never abort
/// sibling entries; only a text that is neither valid JSON nor valid YAML at
/// all fails the whole call.
#[server]
pub async fn parse_mcp_snippet(text: String) -> Result<McpSnippetParse, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        parse_snippet_text(&text).map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = text;
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// List every server configured in `scope`'s `mcp_servers` map, each with its
/// live-earned `status` (D-01/D-02/D-20/D-12).
#[server]
pub async fn list_mcp_servers(scope: ConfigScope) -> Result<Vec<McpServerRow>, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (config, _target) = resolve_scope_target(&scope).map_err(ServerFnError::new)?;
        let mut rows = Vec::new();
        for name in config.mcp_servers.keys() {
            rows.push(row_for(name, &config).await.map_err(ServerFnError::new)?);
        }
        Ok(rows)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scope;
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Read `scope`'s live-earned status for `name` alone (D-12). Consults
/// [`OAUTH_ATTEMPTS`] FIRST (Task 4/48.2-09): `InFlight` and `Finalizing`
/// both report [`McpServerStatus::Connecting`] — deliberately changed from
/// the pre-48.2-09 mapping of `InFlight` to `AuthRequired`. Under the old
/// stub, `InFlight` meant "we are pretending to try", and `AuthRequired` was
/// the closest honest answer available; now it means the discovery/DCR round
/// trip (or, for `Finalizing`, the post-callback manager reload) is
/// genuinely running — a connecting state — and `AuthRequired` is reserved
/// for its real meaning: no cached token, nobody is authorizing.
/// `AwaitingAuthorization` reports the real authorization URL. A `Failed`
/// attempt reports `SpawnFailed` carrying the recorded reason, and a
/// `Succeeded` attempt clears itself and falls through to the live
/// classification below. Otherwise, every status read is produced from a
/// fresh config read and, when a live manager is installed, from the
/// manager's CURRENT connected set — a returned `Connected` is a fact the
/// manager itself reported, never assumed.
#[server]
pub async fn mcp_server_status(
    scope: ConfigScope,
    name: String,
) -> Result<McpServerStatus, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let key = oauth_attempt_key(&scope, &name);
        {
            let mut attempts = oauth_attempts().lock().await;
            // Self-healing staleness check, covering all three non-terminal
            // states: `started_at` bounds how long a poller will keep
            // believing an attempt is still running. `connect_http_oauth`'s
            // own loopback wait is bounded at 300s (transport.rs) — this
            // ceiling is set comfortably above that so a normal
            // authorization is never mistaken for stale, while a task that
            // somehow never recorded its outcome (a panic that unwound past
            // the recording step, for example) does not leave the UI
            // reporting a non-terminal status forever.
            let non_terminal_started_at = match attempts.get(&key) {
                Some(OAuthAttemptState::InFlight { started_at }) => Some(*started_at),
                Some(OAuthAttemptState::AwaitingAuthorization { started_at, .. }) => {
                    Some(*started_at)
                }
                Some(OAuthAttemptState::Finalizing { started_at }) => Some(*started_at),
                _ => None,
            };
            if let Some(started_at) = non_terminal_started_at {
                const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(360);
                if started_at.elapsed() > STALE_AFTER {
                    attempts.insert(
                        key.clone(),
                        OAuthAttemptState::Failed {
                            reason: "OAuth CONNECT attempt timed out".to_string(),
                        },
                    );
                    return Ok(McpServerStatus::SpawnFailed {
                        reason: "OAuth CONNECT attempt timed out".to_string(),
                    });
                }
            }
            match attempts.get(&key) {
                Some(OAuthAttemptState::InFlight { .. }) => {
                    return Ok(McpServerStatus::Connecting);
                }
                Some(OAuthAttemptState::AwaitingAuthorization { auth_url, .. }) => {
                    return Ok(McpServerStatus::AwaitingAuthorization {
                        auth_url: auth_url.clone(),
                    });
                }
                Some(OAuthAttemptState::Finalizing { .. }) => {
                    return Ok(McpServerStatus::Connecting);
                }
                Some(OAuthAttemptState::Failed { reason }) => {
                    return Ok(McpServerStatus::SpawnFailed {
                        reason: reason.clone(),
                    });
                }
                Some(OAuthAttemptState::Succeeded) => {
                    attempts.remove(&key);
                }
                None => {}
            }
        }
        let (config, _target) = resolve_scope_target(&scope).map_err(ServerFnError::new)?;
        let raw = config
            .mcp_servers
            .get(&name)
            .ok_or_else(|| ServerFnError::new(format!("unknown server: {name}")))?;
        let cfg = server_config_from_value(raw).map_err(ServerFnError::new)?;
        Ok(classify_status_live(&name, &cfg).await)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, name);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Start a non-blocking OAuth CONNECT attempt for `name` (Task 4/48.2-09,
/// D-03/RESEARCH Pitfall 3). Returns within ~1 second regardless of how long
/// the underlying authorization takes — a real handshake blocks on AS
/// discovery and DCR; awaiting it inline would hold an axum worker (and the
/// browser's own fetch) open for that entire window. The spawned task
/// records its outcome in [`OAUTH_ATTEMPTS`], pollable via
/// [`mcp_server_status`]: on success it parks `AwaitingAuthorization` with
/// the real auth URL and indexes the OAuth `state` (Task 2 consumes this);
/// on failure it records `Failed`. Live-apply reload no longer happens from
/// this function (or its spawned task) — CONNECTED is now earned only after
/// the web callback completes the handshake and reloads the manager
/// (Task 2/48.2-09), so firing a bulk reload here would bounce every other
/// MCP server for an authorization that has not even started yet.
///
/// `browser_origin` (D-09/T-48.2-09-02) is the browser's own origin, used to
/// build the OAuth redirect URI ONLY when `mcp_oauth.web_redirect_base_url`
/// is unset — the operator-configured value always wins when present and
/// non-blank. Either candidate must pass
/// `ironhermes_mcp::security::validate_web_redirect_base`; CONNECT already
/// requires an authenticated session and `check_mcp_write_gate()` before
/// this value is ever used, and the PKCE verifier never leaves this process
/// regardless of which origin wins (see this plan's threat register,
/// T-48.2-09-02).
#[server]
pub async fn connect_mcp_oauth(
    scope: ConfigScope,
    name: String,
    browser_origin: String,
) -> Result<(), ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (config, _target) = resolve_scope_target(&scope).map_err(ServerFnError::new)?;
        // D-10: the write gate is checked before any manager call and before
        // any network call — first, unconditionally.
        check_mcp_write_gate().map_err(ServerFnError::new)?;
        let raw = config
            .mcp_servers
            .get(&name)
            .ok_or_else(|| ServerFnError::new(format!("unknown server: {name}")))?;
        let cfg = server_config_from_value(raw).map_err(ServerFnError::new)?;
        if cfg.oauth_provider.is_none() {
            return Err(ServerFnError::new(format!(
                "server '{name}' has no oauth_provider configured"
            )));
        }

        let configured_base = root_web_redirect_base_url();
        let redirect_base = resolve_redirect_base(configured_base.as_deref(), &browser_origin)
            .map_err(ServerFnError::new)?;
        let redirect_uri = format!("{redirect_base}{MCP_OAUTH_CALLBACK_PATH}");

        let key = oauth_attempt_key(&scope, &name);
        {
            let mut attempts = oauth_attempts().lock().await;
            if matches!(
                attempts.get(&key),
                Some(
                    OAuthAttemptState::InFlight { .. }
                        | OAuthAttemptState::AwaitingAuthorization { .. }
                        | OAuthAttemptState::Finalizing { .. }
                )
            ) {
                return Err(ServerFnError::new(format!(
                    "a connect attempt for '{name}' is already in flight"
                )));
            }
            attempts.insert(
                key,
                OAuthAttemptState::InFlight {
                    started_at: std::time::Instant::now(),
                },
            );
        }

        // UAT round 2 (Task 5 fix A): the background connect task is wrapped
        // in `oauth_connect_watchdog`, which nested-spawns the authorization
        // start, bounds it with `OAUTH_CONNECT_TIMEOUT` (inside
        // `perform_oauth_connect`), and records a terminal state through
        // `record_oauth_connect_outcome` on every exit — success, ordinary
        // error, timeout, panic, or cancellation. `InFlight` (inserted just
        // above) is therefore never left stuck: every code path out of this
        // spawn writes a terminal or awaiting state.
        let watchdog_scope = scope.clone();
        let watchdog_name = name.clone();
        let inner_name = name.clone();
        tokio::spawn(oauth_connect_watchdog(watchdog_scope, watchdog_name, async move {
            perform_oauth_connect(&inner_name, &cfg, &redirect_uri).await
        }));

        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, name, browser_origin);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Probe a draft with a real handshake through `ironhermes-mcp`'s own
/// transport, without writing `config.yaml` (D-04). A write-class action
/// (T-48.2-02-01) — the write gate is checked before any spawn/request.
#[server]
pub async fn probe_mcp_server(
    scope: ConfigScope,
    draft: McpServerDraft,
) -> Result<McpProbeResult, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (_config, _target) = resolve_scope_target(&scope).map_err(ServerFnError::new)?;
        check_mcp_write_gate().map_err(ServerFnError::new)?;
        let cfg = draft_to_server_config(&draft);
        Ok(run_probe(&draft.name, &cfg).await)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, draft);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Commit a draft into `scope`'s `config.yaml`, then live-apply and return
/// the freshly classified row (D-01/D-02/D-12/D-13). The probed config IS the
/// committed config, byte-identical — this always writes through
/// [`write_server_into`]'s `serde_yaml::to_value`, never a hand-built
/// fragment.
#[server]
pub async fn commit_mcp_server(
    scope: ConfigScope,
    draft: McpServerDraft,
) -> Result<McpServerRow, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        // CR-01: enforce the same round-trip check `parse_one_entry` already
        // performs on the paste-import path, at the actual write boundary —
        // `commit_mcp_server` is reached from BOTH the paste path and the
        // manual DEFINE-form path, and only the former was previously
        // checked.
        if ironhermes_mcp::sanitize_server_name(&draft.name) != draft.name {
            return Err(ServerFnError::new(format!(
                "server name '{}' contains characters outside [A-Za-z0-9_]; \
                 rename it before committing",
                draft.name
            )));
        }
        let (mut config, target) = resolve_scope_target(&scope).map_err(ServerFnError::new)?;
        check_mcp_write_gate().map_err(ServerFnError::new)?;
        // CR-01: reject a name that collides with a DIFFERENT
        // already-configured server's sanitized form. `mcp_group_server_key`
        // is the only lookup from an `mcp__<sanitized>` display group back to
        // a real `mcp_servers` key — `HashMap::keys()` iteration order is
        // unspecified, so an undetected collision here lets an operator's
        // toggle silently act on the wrong server. Re-committing the SAME
        // name is a supported update path and must still succeed.
        if let Some(existing) = crate::server::tools_config_api::mcp_group_server_key(
            &format!("mcp__{}", ironhermes_mcp::sanitize_server_name(&draft.name)),
            &config.mcp_servers,
        ) {
            if existing != draft.name {
                return Err(ServerFnError::new(format!(
                    "server name '{}' collides with the already-configured server '{existing}' \
                     after sanitization; choose a different name",
                    draft.name
                )));
            }
        }
        let cfg = draft_to_server_config(&draft);
        write_server_into(&mut config, &draft.name, &cfg).map_err(ServerFnError::new)?;
        save_scoped(&config, &target).map_err(ServerFnError::new)?;
        reload_mcp_and_report(&scope, &config).await;
        row_for(&draft.name, &config)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, draft);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Flip a configured server's `enabled` field, save, live-apply, and return
/// the freshly classified row (D-12).
#[server]
pub async fn set_mcp_server_enabled(
    scope: ConfigScope,
    name: String,
    enabled: bool,
) -> Result<McpServerRow, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (mut config, target) = resolve_scope_target(&scope).map_err(ServerFnError::new)?;
        check_mcp_write_gate().map_err(ServerFnError::new)?;
        let raw = config
            .mcp_servers
            .get(&name)
            .cloned()
            .ok_or_else(|| ServerFnError::new(format!("unknown server: {name}")))?;
        let mut cfg = server_config_from_value(&raw).map_err(ServerFnError::new)?;
        cfg.enabled = enabled;
        write_server_into(&mut config, &name, &cfg).map_err(ServerFnError::new)?;
        save_scoped(&config, &target).map_err(ServerFnError::new)?;
        reload_mcp_and_report(&scope, &config).await;
        row_for(&name, &config).await.map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, name, enabled);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Remove a configured server from `scope`'s `config.yaml`, save, and
/// live-apply (D-12/D-13).
#[server]
pub async fn remove_mcp_server(scope: ConfigScope, name: String) -> Result<(), ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (mut config, target) = resolve_scope_target(&scope).map_err(ServerFnError::new)?;
        check_mcp_write_gate().map_err(ServerFnError::new)?;
        config.mcp_servers.remove(&name);
        save_scoped(&config, &target).map_err(ServerFnError::new)?;
        reload_mcp_and_report(&scope, &config).await;
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, name);
        unreachable!("server fn body never runs on the wasm client")
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // <behavior> case 1: Claude-Desktop-style JSON block.
    // -------------------------------------------------------------------

    #[test]
    fn behavior_1_claude_desktop_json_parses_to_stdio_draft() {
        let text = r#"{"mcpServers": {"github": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"], "env": {"X": "1"}}}}"#;
        let result = parse_snippet_text(text).expect("valid JSON must parse");
        assert_eq!(result.entries.len(), 1);
        let entry = &result.entries[0];
        assert_eq!(entry.name, "github");
        assert!(entry.error.is_none());
        let draft = entry.draft.as_ref().expect("draft must be present");
        assert_eq!(draft.transport, McpTransportKind::Stdio);
        assert_eq!(draft.command.as_deref(), Some("npx"));
        assert_eq!(
            draft.args,
            vec!["-y".to_string(), "@modelcontextprotocol/server-github".to_string()]
        );
        assert!(draft.env.iter().any(|(k, v)| k == "X" && v == "1"));
    }

    // -------------------------------------------------------------------
    // <behavior> case 2: raw YAML fragment.
    // -------------------------------------------------------------------

    #[test]
    fn behavior_2_raw_yaml_fragment_parses_to_http_draft() {
        let text = "mcp_servers:\n  docs:\n    url: https://example.test/mcp\n";
        let result = parse_snippet_text(text).expect("valid YAML must parse");
        assert_eq!(result.entries.len(), 1);
        let entry = &result.entries[0];
        assert_eq!(entry.name, "docs");
        assert!(entry.error.is_none());
        let draft = entry.draft.as_ref().expect("draft must be present");
        assert_eq!(draft.transport, McpTransportKind::Http);
        assert_eq!(draft.url.as_deref(), Some("https://example.test/mcp"));
        assert!(draft.command.is_none());
    }

    // -------------------------------------------------------------------
    // <behavior> case 3: multi-server partial-parse isolation.
    // -------------------------------------------------------------------

    #[test]
    fn behavior_3_multi_server_partial_parse_isolates_the_malformed_entry() {
        let text = r#"{
            "mcpServers": {
                "good": {"command": "npx"},
                "bad": {"env": {"TOKEN": "sk-not-a-real-secret"}}
            }
        }"#;
        let result = parse_snippet_text(text).expect("top-level JSON is valid");
        assert_eq!(result.entries.len(), 2, "both entries must be present");
        let good = result
            .entries
            .iter()
            .find(|e| e.name == "good")
            .expect("good entry must exist");
        assert!(good.draft.is_some());
        assert!(good.error.is_none());
        let bad = result
            .entries
            .iter()
            .find(|e| e.name == "bad")
            .expect("bad entry must exist");
        assert!(bad.draft.is_none());
        assert!(bad.error.is_some());
    }

    // -------------------------------------------------------------------
    // <behavior> case 4: neither command nor url.
    // -------------------------------------------------------------------

    #[test]
    fn behavior_4_entry_with_neither_command_nor_url_names_the_missing_field() {
        let text = r#"{"mcpServers": {"nothing": {}}}"#;
        let result = parse_snippet_text(text).expect("top-level JSON is valid");
        let entry = &result.entries[0];
        assert!(entry.draft.is_none());
        let error = entry.error.as_ref().expect("error must be present");
        assert!(
            error.contains("command") && error.contains("url"),
            "error must name the missing field(s); got: {error}"
        );
    }

    // -------------------------------------------------------------------
    // <behavior> case 5: draft -> to_value -> from_value round-trip.
    // -------------------------------------------------------------------

    #[test]
    fn behavior_5_draft_round_trips_through_serde_yaml_value_including_oauth_fields() {
        let draft = McpServerDraft {
            name: "cf_docs".to_string(),
            transport: McpTransportKind::Http,
            command: None,
            args: Vec::new(),
            env: Vec::new(),
            url: Some("https://docs.mcp.cloudflare.com/mcp".to_string()),
            headers: vec![("X-Test".to_string(), "1".to_string())],
            enabled: true,
            timeout: 120,
            connect_timeout: 60,
            oauth_provider: Some("cloudflare_mcp_docs".to_string()),
            allowed_issuer: Some("mcp.cloudflare.com".to_string()),
        };
        let cfg = draft_to_server_config(&draft);
        let value = serde_yaml::to_value(&cfg).expect("to_value must succeed");
        let round_tripped: ironhermes_mcp::McpServerConfig =
            serde_yaml::from_value(value).expect("from_value must succeed");

        assert_eq!(round_tripped.url, cfg.url);
        assert_eq!(round_tripped.headers, cfg.headers);
        assert_eq!(round_tripped.enabled, cfg.enabled);
        assert_eq!(round_tripped.timeout, cfg.timeout);
        assert_eq!(round_tripped.connect_timeout, cfg.connect_timeout);
        assert_eq!(
            round_tripped.oauth_provider, cfg.oauth_provider,
            "oauth_provider must survive the round-trip"
        );
        assert_eq!(
            round_tripped.allowed_issuer, cfg.allowed_issuer,
            "allowed_issuer must survive the round-trip"
        );
    }

    // -------------------------------------------------------------------
    // <behavior> case 6: commit into Config, save, reload, read back equal.
    // -------------------------------------------------------------------

    #[test]
    fn behavior_6_committed_draft_round_trips_byte_equal_through_config_save_and_load() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let draft = McpServerDraft {
            name: "github".to_string(),
            transport: McpTransportKind::Stdio,
            command: Some("npx".to_string()),
            args: vec!["-y".to_string(), "@modelcontextprotocol/server-github".to_string()],
            env: vec![("GITHUB_TOKEN".to_string(), "ghp_test".to_string())],
            url: None,
            headers: Vec::new(),
            enabled: true,
            timeout: 120,
            connect_timeout: 60,
            oauth_provider: None,
            allowed_issuer: None,
        };
        let cfg = draft_to_server_config(&draft);

        let mut config = ironhermes_core::config::Config::default();
        write_server_into(&mut config, "github", &cfg).expect("write must succeed");
        config.save().expect("save must succeed");

        let reloaded = ironhermes_core::config::Config::load().expect("load must succeed");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        let raw = reloaded
            .mcp_servers
            .get("github")
            .expect("committed server must be present after reload");
        let read_back = server_config_from_value(raw).expect("read-back must parse");

        assert_eq!(read_back.command, cfg.command);
        assert_eq!(read_back.args, cfg.args);
        assert_eq!(read_back.env, cfg.env);
        assert_eq!(read_back.enabled, cfg.enabled);
        assert_eq!(read_back.oauth_provider, cfg.oauth_provider);
        assert_eq!(read_back.allowed_issuer, cfg.allowed_issuer);
    }

    // -------------------------------------------------------------------
    // Acceptance-criteria-specific tests.
    // -------------------------------------------------------------------

    /// A server name that does not survive `sanitize_server_name` unchanged
    /// (contains `/` or `:`) is rejected as that entry's parse error, and its
    /// sibling entries still parse.
    #[test]
    fn server_name_failing_sanitize_round_trip_is_rejected_siblings_still_parse() {
        let text = r#"{
            "mcpServers": {
                "https://mcp.example.com/docs": {"url": "https://mcp.example.com/docs"},
                "clean_name": {"url": "https://example.test/mcp"}
            }
        }"#;
        let result = parse_snippet_text(text).expect("top-level JSON is valid");
        assert_eq!(result.entries.len(), 2);
        let bad = result
            .entries
            .iter()
            .find(|e| e.name == "https://mcp.example.com/docs")
            .expect("bad-name entry must exist");
        assert!(bad.draft.is_none());
        assert!(bad.error.is_some());
        let good = result
            .entries
            .iter()
            .find(|e| e.name == "clean_name")
            .expect("sibling entry must still parse");
        assert!(good.draft.is_some());
    }

    /// A snippet whose `env` contains a sentinel secret value produces an
    /// error string (for the malformed sibling entry) that does not contain
    /// that sentinel.
    #[test]
    fn malformed_entry_error_never_contains_a_planted_env_secret() {
        const SENTINEL: &str = "sk-planted-sentinel-secret-value-12345";
        let text = format!(
            r#"{{
                "mcpServers": {{
                    "bad": {{"env": {{"TOKEN": "{SENTINEL}"}}}}
                }}
            }}"#
        );
        let result = parse_snippet_text(&text).expect("top-level JSON is valid");
        let entry = &result.entries[0];
        let error = entry.error.as_ref().expect("entry must be an error (no command/url)");
        assert!(
            !error.contains(SENTINEL),
            "error string must never contain the planted sentinel secret; got: {error}"
        );
    }

    /// A malformed multi-entry paste round-trips through `list_mcp_servers`'s
    /// sibling read path without a committed, well-formed server disappearing.
    #[test]
    fn draft_to_server_config_leaves_unset_fields_as_none() {
        let draft = McpServerDraft {
            name: "minimal".to_string(),
            transport: McpTransportKind::Stdio,
            command: Some("echo".to_string()),
            args: Vec::new(),
            env: Vec::new(),
            url: None,
            headers: Vec::new(),
            enabled: true,
            timeout: 120,
            connect_timeout: 60,
            oauth_provider: None,
            allowed_issuer: None,
        };
        let cfg = draft_to_server_config(&draft);
        assert!(cfg.url.is_none());
        assert!(cfg.oauth_provider.is_none());
        assert!(cfg.allowed_issuer.is_none());
        assert!(cfg.enabled_tools.is_none());
        assert!(cfg.auth.is_none());
        assert!(cfg.sampling.is_none());
    }

    // =====================================================================
    // Task 3: classify_server_status, probe_mcp_server, commit_mcp_server
    // =====================================================================

    fn mk_cfg(enabled: bool, oauth_provider: Option<&str>) -> ironhermes_mcp::McpServerConfig {
        ironhermes_mcp::McpServerConfig {
            enabled,
            oauth_provider: oauth_provider.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    /// Task 3 <behavior> case 1: `Disabled` regardless of any manager state.
    #[test]
    fn task3_behavior_1_disabled_config_is_disabled_regardless_of_manager_state() {
        let cfg = mk_cfg(false, None);
        let mut connected = std::collections::HashSet::new();
        connected.insert("srv".to_string());
        let status = classify_server_status("srv", &cfg, &connected, None, true);
        assert_eq!(status, McpServerStatus::Disabled);
    }

    /// Task 3 <behavior> case 2: OAuth + no token classifies `AuthRequired`,
    /// even when the bulk start result buckets the server as failed.
    #[test]
    fn task3_behavior_2_oauth_no_token_is_auth_required_not_spawn_failed() {
        let cfg = mk_cfg(true, Some("cloudflare_api"));
        let connected = std::collections::HashSet::new();
        let status = classify_server_status(
            "srv",
            &cfg,
            &connected,
            Some("connection failed after retries"),
            false,
        );
        assert_eq!(status, McpServerStatus::AuthRequired);
    }

    /// Task 3 <behavior> case 3: `Connected` for an enabled server present in
    /// the manager's connected set.
    #[test]
    fn task3_behavior_3_enabled_and_connected_is_connected() {
        let cfg = mk_cfg(true, None);
        let mut connected = std::collections::HashSet::new();
        connected.insert("srv".to_string());
        let status = classify_server_status("srv", &cfg, &connected, None, false);
        assert_eq!(status, McpServerStatus::Connected);
    }

    /// Task 3 <behavior> case 4: `SpawnFailed` with the manager's sanitized
    /// reason for an enabled non-OAuth server absent from the connected set.
    #[test]
    fn task3_behavior_4_enabled_non_oauth_not_connected_is_spawn_failed_with_reason() {
        let cfg = mk_cfg(true, None);
        let connected = std::collections::HashSet::new();
        let status =
            classify_server_status("srv", &cfg, &connected, Some("connection refused"), false);
        assert_eq!(
            status,
            McpServerStatus::SpawnFailed {
                reason: "connection refused".to_string()
            }
        );
    }

    /// Acceptance criterion: OAuth + no token + a manager failure reason
    /// classifies `AuthRequired`, NOT `SpawnFailed` — the precedence check
    /// spelled out explicitly (distinct from case 2 above, which uses the
    /// same inputs; kept as its own named test per the acceptance criteria
    /// wording).
    #[test]
    fn oauth_no_token_with_failure_reason_present_is_still_auth_required() {
        let cfg = mk_cfg(true, Some("cloudflare_api"));
        let connected = std::collections::HashSet::new();
        let status = classify_server_status(
            "srv",
            &cfg,
            &connected,
            Some("connection failed after retries"),
            false,
        );
        assert_ne!(
            status,
            McpServerStatus::SpawnFailed {
                reason: "connection failed after retries".to_string()
            }
        );
        assert_eq!(status, McpServerStatus::AuthRequired);
    }

    /// Warm-but-revoked follow-up fix (root-caused after Task 5's UAT
    /// failure): a token IS present (`has_token: true`, unlike case 2/the
    /// cold-start test above) but the server is not connected and the live
    /// failure reason is the exact text `transport.rs`'s hot path produces
    /// when a cached token's refresh fails against a revoked/expired grant.
    /// This MUST classify `AuthRequired` — the same CONNECT -> AUTHORIZE
    /// affordance a never-authorized server gets — not `SpawnFailed`.
    ///
    /// This test fails against the pre-fix `classify_server_status` (which
    /// gated the AuthRequired branch on `!has_token` alone, ignoring
    /// `failure_reason` entirely): with `has_token: true` the old `if
    /// cfg.oauth_provider.is_some() && !has_token` guard is false, so
    /// execution falls through to `connected.contains(name)` (false, empty
    /// set) and returns `SpawnFailed { reason: "Get access token: ..." }`
    /// instead of `AuthRequired` — this assertion would fail. The fix adds
    /// the `auth_caused_failure` branch that makes it pass.
    #[test]
    fn warm_but_revoked_oauth_token_present_but_dead_is_auth_required_not_spawn_failed() {
        let cfg = mk_cfg(true, Some("cloudflare_api"));
        let connected = std::collections::HashSet::new();
        let status = classify_server_status(
            "srv",
            &cfg,
            &connected,
            Some("Get access token: token refresh failed: invalid_grant: Grant not found"),
            true, // has_token: true — the token IS present, just dead
        );
        assert_eq!(status, McpServerStatus::AuthRequired);
    }

    /// Sibling negative case for the same fix: a token is present, the
    /// server is not connected, but the failure reason is a GENUINE
    /// transport/spawn failure with no auth-related marker. This must NOT be
    /// faked into `AuthRequired` — "do not classify as AuthRequired merely
    /// because a server is not connected" (fix requirement). It must still
    /// read as `SpawnFailed`, distinguishing the two causes.
    #[test]
    fn oauth_token_present_genuine_transport_failure_is_still_spawn_failed() {
        let cfg = mk_cfg(true, Some("cloudflare_api"));
        let connected = std::collections::HashSet::new();
        let status = classify_server_status(
            "srv",
            &cfg,
            &connected,
            Some("connection refused"),
            true, // has_token: true, and the failure is NOT auth-caused
        );
        assert_eq!(
            status,
            McpServerStatus::SpawnFailed {
                reason: "connection refused".to_string()
            }
        );
    }

    /// Task 3 <behavior> case 5: a stdio draft pointing at a nonexistent
    /// command returns a failed probe whose message does not contain the
    /// process environment.
    #[tokio::test]
    async fn task3_behavior_5_probe_nonexistent_stdio_command_fails_without_leaking_env() {
        let cfg = ironhermes_mcp::McpServerConfig {
            command: Some("this-binary-does-not-exist-anywhere-on-path".to_string()),
            env: [("SUPER_SECRET_ENV".to_string(), "leak-me-not".to_string())]
                .into_iter()
                .collect(),
            connect_timeout: 2,
            ..Default::default()
        };
        let result = run_probe("nonexistent", &cfg).await;
        assert!(!result.passed);
        let message = result.message.expect("failed probe must carry a message");
        assert!(!message.contains("leak-me-not"));
        assert!(!message.contains("SUPER_SECRET_ENV"));
    }

    /// Task 3 <behavior> case 6: an OAuth-configured HTTP draft with no
    /// cached token returns a probe result marked as a pass with status
    /// `AuthRequired` and an empty discovered-tool list.
    #[tokio::test]
    async fn task3_behavior_6_oauth_draft_probe_passes_as_auth_required_with_no_tools() {
        let cfg = ironhermes_mcp::McpServerConfig {
            url: Some("https://mcp.cloudflare.com/mcp".to_string()),
            oauth_provider: Some("cloudflare_api".to_string()),
            ..Default::default()
        };
        let result = run_probe("cf", &cfg).await;
        assert!(result.passed);
        assert_eq!(result.status, McpServerStatus::AuthRequired);
        assert!(result.tools.is_empty());
    }

    /// Task 3 <behavior> case 7 (structural): no code path inside this
    /// module's probe machinery calls a registry register/unregister
    /// function — mirrors `server_task.rs`'s own `include_str!` regression
    /// pattern. Proves by construction that a probe can never leak into the
    /// live `ToolRegistry` (REVIEWS finding 3 / T-48.2-02-10): `run_probe`
    /// and `connect_and_list_non_oauth` never receive a registry parameter
    /// at all.
    #[test]
    fn task3_behavior_7_probe_machinery_never_touches_a_tool_registry() {
        let src = include_str!("mcp_admin_api.rs");
        // Scan only the probe-specific functions, not the whole file (whose
        // doc comments legitimately mention "register"/"unregister" in
        // prose describing what NOT to do).
        let start = src
            .find("async fn connect_and_list_non_oauth")
            .expect("connect_and_list_non_oauth must exist");
        let end = src
            .find("pub async fn parse_mcp_snippet")
            .expect("parse_mcp_snippet must exist");
        let probe_region = &src[start..end];
        assert!(
            !probe_region.contains("register_dynamic") && !probe_region.contains("unregister_by_prefix"),
            "probe machinery must never call a registry register/unregister fn"
        );
    }

    /// Task 3 <behavior> case 8: after a stdio probe of a long-lived command
    /// (a hung, non-MCP-speaking `sh` process that first records its own PID
    /// to a file) returns, that child process is no longer running — on the
    /// failure path (the process never speaks MCP, so the probe's bounded
    /// `connect_timeout` fires and tears it down). `sh`'s `exec sleep 300`
    /// keeps the recorded PID stable across the exec.
    #[cfg(unix)]
    #[tokio::test]
    async fn task3_behavior_8_probe_kills_long_lived_stdio_child_after_timeout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("pid");
        let cfg = ironhermes_mcp::McpServerConfig {
            command: Some("sh".to_string()),
            args: vec![
                "-c".to_string(),
                format!("echo $$ > {}; exec sleep 300", pid_file.display()),
            ],
            connect_timeout: 1,
            ..Default::default()
        };

        let result = run_probe("hang", &cfg).await;
        assert!(!result.passed, "a non-MCP-speaking process must fail the probe");

        // The pid file is written almost immediately at shell startup, well
        // before the 1s connect_timeout elapses — poll briefly for safety.
        let mut pid_text = String::new();
        for _ in 0..20 {
            if let Ok(text) = std::fs::read_to_string(&pid_file) {
                if !text.trim().is_empty() {
                    pid_text = text;
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let pid = pid_text.trim();
        assert!(!pid.is_empty(), "pid file must have been written by the probed process");

        // `kill -0 <pid>` succeeds iff the process still exists — poll
        // briefly since SIGKILL delivery/reaping is not instantaneous.
        let mut still_alive = true;
        for _ in 0..20 {
            let status = std::process::Command::new("kill")
                .args(["-0", pid])
                .status()
                .expect("kill -0 must be runnable");
            if !status.success() {
                still_alive = false;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            !still_alive,
            "GAP-style leak: probed stdio child (pid {pid}) must be dead after probe_mcp_server returns"
        );
    }

    /// Task 3 <behavior> case 9: `commit_mcp_server`-equivalent write path
    /// with the write gate closed returns an error and leaves
    /// `config.mcp_servers` on disk unchanged.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn task3_behavior_9_commit_with_gate_closed_leaves_disk_unchanged() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        // Default config: web_config_write_enabled is false.
        let cfg = ironhermes_core::config::Config::default();
        cfg.save().expect("seed root config.yaml");
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let (mut config, target) = resolve_scope_target(&ConfigScope::Root).expect("resolve");
        let commit_result = match check_mcp_write_gate() {
            Err(e) => Err(e),
            Ok(()) => {
                let draft_cfg = draft_to_server_config(&McpServerDraft {
                    name: "github".to_string(),
                    transport: McpTransportKind::Stdio,
                    command: Some("npx".to_string()),
                    args: Vec::new(),
                    env: Vec::new(),
                    url: None,
                    headers: Vec::new(),
                    enabled: true,
                    timeout: 120,
                    connect_timeout: 60,
                    oauth_provider: None,
                    allowed_issuer: None,
                });
                write_server_into(&mut config, "github", &draft_cfg)
                    .and_then(|()| save_scoped(&config, &target))
            }
        };

        let after = std::fs::read(&config_path).expect("read config after gated commit");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(commit_result.is_err(), "gate-closed commit must error");
        assert_eq!(before, after, "on-disk config bytes must be unchanged when the gate is closed");
    }

    // =====================================================================
    // Task 4: connect_mcp_oauth — non-blocking OAuth CONNECT
    // =====================================================================

    /// Task 4/48.2-09 <behavior> case 1: returns within one second for a
    /// server whose OAuth flow would (in a real build) take minutes — proven
    /// here against a process with no `AppState` installed, so
    /// `perform_oauth_connect` fast-fails on the missing-manager branch
    /// rather than attempting any network I/O.
    #[tokio::test]
    async fn task4_behavior_1_perform_oauth_connect_returns_quickly() {
        let cfg = ironhermes_mcp::McpServerConfig {
            oauth_provider: Some("cloudflare_api".to_string()),
            ..Default::default()
        };
        let start = std::time::Instant::now();
        let result = perform_oauth_connect("cf", &cfg, "https://hermes.example.com/oauth/mcp/callback").await;
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "perform_oauth_connect must never block on network I/O"
        );
        assert!(
            result.is_err(),
            "no AppState is installed in this unit-test process, so this must fast-fail"
        );
    }

    // =====================================================================
    // UAT round 2 (Task 5 fix A): the authorization-start path must be
    // bounded by OAUTH_CONNECT_TIMEOUT and every exit from the background
    // connect task — timeout included, panic included — must clear InFlight
    // to a terminal state. Unlike `task4_behavior_1` above (which only
    // exercises the missing-manager fast-fail branch and returns in well
    // under a second), these tests exercise the code path that actually
    // waits: a future that never resolves, standing in for a hung
    // `manager.begin_oauth` network call.
    // =====================================================================

    /// A short real duration used only by these tests, so the timeout path
    /// actually fires (and this test file actually waits for it) in well
    /// under a second rather than requiring `tokio::time::pause` / the
    /// `test-util` feature (not enabled for this crate's dev-dependencies)
    /// or a real 30-second sleep.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    const TEST_OAUTH_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(20);

    /// The timeout wrapper itself fires and returns `Err` when given a
    /// future that never resolves — proving the bound exists independently
    /// of `perform_oauth_connect`'s missing-manager branch. Uses
    /// `await_with_oauth_timeout_after` with a millisecond-scale duration so
    /// the test genuinely waits out a real timeout rather than mocking one.
    #[tokio::test]
    async fn bugfix_a_await_with_oauth_timeout_fires_on_a_hung_future() {
        let never = std::future::pending::<Result<(String, String), String>>();
        let result =
            await_with_oauth_timeout_after(TEST_OAUTH_TIMEOUT, never, || "authorization start timed out".to_string())
                .await;
        assert!(
            result.is_err(),
            "a hung authorization-start future must time out, not hang forever"
        );
    }

    /// End-to-end for the timeout path: an `InFlight` attempt, driven
    /// through the exact `await_with_oauth_timeout_after` -> `record_oauth_connect_outcome`
    /// pipeline `perform_oauth_connect`/`oauth_connect_watchdog` use, ends up
    /// `Failed` rather than stuck `InFlight` — this is the regression this
    /// fix closes (a live capture showed an idle, half-open TCP connection
    /// to the authorization server while the row's pill spun forever).
    #[tokio::test]
    async fn bugfix_a_timeout_path_clears_inflight_to_failed() {
        let key = oauth_attempt_key(&ConfigScope::Root, "hung-timeout-server");
        oauth_attempts().lock().await.insert(
            key.clone(),
            OAuthAttemptState::InFlight {
                started_at: std::time::Instant::now(),
            },
        );

        let never = std::future::pending::<Result<(String, String), String>>();
        let outcome =
            await_with_oauth_timeout_after(TEST_OAUTH_TIMEOUT, never, || "authorization start timed out".to_string())
                .await;
        assert!(outcome.is_err(), "precondition: the hung future must have timed out");

        record_oauth_connect_outcome(ConfigScope::Root, "hung-timeout-server".to_string(), outcome).await;

        let attempts = oauth_attempts().lock().await;
        match attempts.get(&key) {
            Some(OAuthAttemptState::Failed { .. }) => {}
            other => panic!("expected the timeout path to clear InFlight to Failed, got {other:?}"),
        }
        drop(attempts);
        oauth_attempts().lock().await.remove(&key);
    }

    /// A panic inside the authorization-start future must not leave the
    /// attempt stuck `InFlight` forever either — `oauth_connect_watchdog`'s
    /// nested `tokio::spawn` + `JoinHandle` await converts the panic into an
    /// ordinary `JoinError`, which is recorded as `Failed` just like any
    /// other error. Tokio isolates the panic to the inner spawned task, so
    /// this test's own pass/fail is driven by the assertions below, not by
    /// the panic propagating out.
    #[tokio::test]
    async fn bugfix_a_panicking_connect_task_clears_inflight_to_failed() {
        let key = oauth_attempt_key(&ConfigScope::Root, "panic-server");
        oauth_attempts().lock().await.insert(
            key.clone(),
            OAuthAttemptState::InFlight {
                started_at: std::time::Instant::now(),
            },
        );

        oauth_connect_watchdog(ConfigScope::Root, "panic-server".to_string(), async {
            panic!("simulated perform_oauth_connect panic (Bug A watchdog regression test)");
            #[allow(unreachable_code)]
            Ok::<(String, String), String>((String::new(), String::new()))
        })
        .await;

        let attempts = oauth_attempts().lock().await;
        match attempts.get(&key) {
            Some(OAuthAttemptState::Failed { .. }) => {}
            other => panic!("expected a panicking connect task to clear InFlight to Failed, got {other:?}"),
        }
        drop(attempts);
        oauth_attempts().lock().await.remove(&key);
    }

    /// Task 4 <behavior> case 2/3: after an attempt is recorded `InFlight`,
    /// `OAUTH_ATTEMPTS` reports it; a second insert attempt while `InFlight`
    /// is rejected by the same guard `connect_mcp_oauth` uses (dedup, no
    /// second spawn).
    #[tokio::test]
    async fn task4_behavior_2_3_in_flight_is_tracked_and_deduped() {
        let key = "test-scope::dedupe-server".to_string();
        {
            let mut attempts = oauth_attempts().lock().await;
            attempts.remove(&key);
        }

        {
            let mut attempts = oauth_attempts().lock().await;
            assert!(
                !matches!(attempts.get(&key), Some(OAuthAttemptState::InFlight { .. })),
                "precondition: no in-flight attempt yet"
            );
            attempts.insert(
                key.clone(),
                OAuthAttemptState::InFlight {
                    started_at: std::time::Instant::now(),
                },
            );
        }

        // Second "call" — the exact guard connect_mcp_oauth uses.
        let already_in_flight = {
            let attempts = oauth_attempts().lock().await;
            matches!(attempts.get(&key), Some(OAuthAttemptState::InFlight { .. }))
        };
        assert!(already_in_flight, "second attempt must observe the in-flight state and be refused");

        {
            let attempts = oauth_attempts().lock().await;
            assert!(
                attempts.get(&key).is_some(),
                "exactly one entry must exist for this key — no duplicate spawn"
            );
        }

        // Cleanup — this map is process-lifetime/shared across tests.
        oauth_attempts().lock().await.remove(&key);
    }

    /// Task 4 <behavior> case 3 (finished outcome, not forever in-flight):
    /// when a spawned attempt finishes with an error, `mcp_server_status`
    /// reports that outcome via `OAUTH_ATTEMPTS` rather than continuing to
    /// report in-flight forever.
    #[tokio::test]
    async fn task4_behavior_3_finished_attempt_is_observable_not_stuck_in_flight() {
        let key = "test-scope::finished-server".to_string();
        {
            let mut attempts = oauth_attempts().lock().await;
            attempts.insert(
                key.clone(),
                OAuthAttemptState::Failed {
                    reason: "authorization denied".to_string(),
                },
            );
        }
        let observed = {
            let attempts = oauth_attempts().lock().await;
            attempts.get(&key).cloned()
        };
        assert!(
            matches!(observed, Some(OAuthAttemptState::Failed { .. })),
            "a finished (failed) attempt must be observable as Failed, not left InFlight"
        );
        oauth_attempts().lock().await.remove(&key);
    }

    /// Task 4 <behavior> case 4: `connect_mcp_oauth` for a server with no
    /// `oauth_provider` configured returns an error without spawning
    /// anything (no entry appears in `OAUTH_ATTEMPTS`).
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn task4_behavior_4_no_oauth_provider_errors_without_spawning() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let cfg = ironhermes_core::config::Config {
            security: ironhermes_core::config::SecurityConfig {
                redact_secrets: true,
                web_config_write_enabled: true,
                web_process_control_enabled: false,
            },
            ..Default::default()
        };
        let draft_cfg = draft_to_server_config(&McpServerDraft {
            name: "no-oauth".to_string(),
            transport: McpTransportKind::Http,
            command: None,
            args: Vec::new(),
            env: Vec::new(),
            url: Some("https://example.test/mcp".to_string()),
            headers: Vec::new(),
            enabled: true,
            timeout: 120,
            connect_timeout: 60,
            oauth_provider: None,
            allowed_issuer: None,
        });
        let mut cfg = cfg;
        write_server_into(&mut cfg, "no-oauth", &draft_cfg).expect("write");
        cfg.save().expect("seed root config.yaml");

        let key = oauth_attempt_key(&ConfigScope::Root, "no-oauth");
        oauth_attempts().lock().await.remove(&key);

        let result = connect_mcp_oauth(
            ConfigScope::Root,
            "no-oauth".to_string(),
            "https://hermes.example.com".to_string(),
        )
        .await;

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "no oauth_provider must error");
        let attempts = oauth_attempts().lock().await;
        assert!(
            !attempts.contains_key(&key),
            "no attempt must have been recorded for a non-OAuth server"
        );
    }

    /// Task 4 <behavior> case 5: `connect_mcp_oauth` with the write gate
    /// closed returns an error without spawning anything.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn task4_behavior_5_gate_closed_errors_without_spawning() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        // Default config: web_config_write_enabled is false.
        let mut cfg = ironhermes_core::config::Config::default();
        let draft_cfg = draft_to_server_config(&McpServerDraft {
            name: "gate-closed-oauth".to_string(),
            transport: McpTransportKind::Http,
            command: None,
            args: Vec::new(),
            env: Vec::new(),
            url: Some("https://mcp.cloudflare.com/mcp".to_string()),
            headers: Vec::new(),
            enabled: true,
            timeout: 120,
            connect_timeout: 60,
            oauth_provider: Some("cloudflare_api".to_string()),
            allowed_issuer: None,
        });
        write_server_into(&mut cfg, "gate-closed-oauth", &draft_cfg).expect("write");
        cfg.save().expect("seed root config.yaml");

        let key = oauth_attempt_key(&ConfigScope::Root, "gate-closed-oauth");
        oauth_attempts().lock().await.remove(&key);

        let result = connect_mcp_oauth(
            ConfigScope::Root,
            "gate-closed-oauth".to_string(),
            "https://hermes.example.com".to_string(),
        )
        .await;

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "gate-closed connect must error");
        let attempts = oauth_attempts().lock().await;
        assert!(
            !attempts.contains_key(&key),
            "no attempt must have been recorded when the gate is closed"
        );
    }

    /// Acceptance criterion: a `Failed` attempt reason recorded from an
    /// error containing a sentinel token value does not itself contain that
    /// sentinel — `perform_oauth_connect` never echoes any input field
    /// (including a planted sentinel in the server's own config) into its
    /// fixed, constructed message. No `AppState` is installed in this
    /// unit-test process, so this exercises the fixed missing-manager
    /// error text, not a live handshake failure.
    #[tokio::test]
    async fn failed_attempt_reason_never_contains_a_planted_sentinel() {
        const SENTINEL: &str = "sk-planted-oauth-sentinel-98765";
        let cfg = ironhermes_mcp::McpServerConfig {
            url: Some(format!("https://example.test/mcp?token={SENTINEL}")),
            oauth_provider: Some(SENTINEL.to_string()),
            ..Default::default()
        };
        let result = perform_oauth_connect(
            "sentinel-server",
            &cfg,
            "https://hermes.example.com/oauth/mcp/callback",
        )
        .await;
        let reason = result.expect_err("no AppState installed in this unit-test process");
        assert!(
            !reason.contains(SENTINEL),
            "Failed reason must never contain a planted sentinel; got: {reason}"
        );
    }

    // =====================================================================
    // Phase 48.2 Plan 09 Task 1(d): resolve_redirect_base precedence + the
    // rejection paths validate_web_redirect_base surfaces through it.
    // =====================================================================

    /// The configured base wins whenever present and non-blank, even when a
    /// (well-formed) browser origin is also supplied.
    #[test]
    fn resolve_redirect_base_prefers_configured_value_when_present() {
        let resolved = resolve_redirect_base(
            Some("https://hermes.example.com"),
            "https://browser-supplied.example.com",
        )
        .expect("both candidates are valid; configured must win");
        assert_eq!(resolved, "https://hermes.example.com");
    }

    /// A blank (whitespace-only) configured value is treated the same as
    /// absent — the browser origin is used instead.
    #[test]
    fn resolve_redirect_base_falls_back_to_browser_origin_when_configured_is_blank() {
        let resolved = resolve_redirect_base(Some("   "), "https://browser-supplied.example.com")
            .expect("blank configured value must fall through to the browser origin");
        assert_eq!(resolved, "https://browser-supplied.example.com");
    }

    /// `None` configured value falls back to the browser origin.
    #[test]
    fn resolve_redirect_base_falls_back_to_browser_origin_when_configured_absent() {
        let resolved = resolve_redirect_base(None, "https://browser-supplied.example.com")
            .expect("absent configured value must fall through to the browser origin");
        assert_eq!(resolved, "https://browser-supplied.example.com");
    }

    /// An invalid configured value is rejected even when the browser origin
    /// would have been valid — the configured value's precedence is absolute
    /// when present and non-blank, it is not a "try this first" fallback.
    #[test]
    fn resolve_redirect_base_rejects_an_invalid_configured_value() {
        let err = resolve_redirect_base(Some("not a url"), "https://browser-supplied.example.com")
            .expect_err("a malformed configured value must be rejected, not silently skipped");
        assert!(err.contains("web redirect base rejected"));
    }

    /// An empty browser origin (the non-wasm `browser_origin()` fallback) is
    /// rejected with the validator's fixed message, not a panic or a
    /// silently-accepted empty base.
    #[test]
    fn resolve_redirect_base_rejects_an_empty_browser_origin_when_unconfigured() {
        let err = resolve_redirect_base(None, "")
            .expect_err("an empty browser origin must be rejected, never silently accepted");
        assert!(err.contains("web redirect base rejected"));
    }

    // =====================================================================
    // Phase 48.2 Plan 05 (D-08 isolation): a profile-scoped MCP commit
    // lands in the profile's config.yaml and leaves root untouched; the
    // live-apply helper is a confirmed no-op at profile scope.
    // =====================================================================

    fn isolation_draft(name: &str) -> McpServerDraft {
        McpServerDraft {
            name: name.to_string(),
            transport: McpTransportKind::Stdio,
            command: Some("npx".to_string()),
            args: Vec::new(),
            env: Vec::new(),
            url: None,
            headers: Vec::new(),
            enabled: true,
            timeout: 120,
            connect_timeout: 60,
            oauth_provider: None,
            allowed_issuer: None,
        }
    }

    /// T-48.2-05-02 sibling for the MCP admin surface: a profile-scoped
    /// `commit_mcp_server` writes the profile's config.yaml and leaves the
    /// root config.yaml BYTES unchanged.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn profile_scoped_mcp_commit_writes_profile_and_leaves_root_byte_identical() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let root_cfg = ironhermes_core::config::Config {
            security: ironhermes_core::config::SecurityConfig {
                redact_secrets: true,
                web_config_write_enabled: true,
                web_process_control_enabled: false,
            },
            ..Default::default()
        };
        root_cfg.save().expect("seed root config.yaml");
        let root_path = ironhermes_core::config::Config::config_path();
        let root_before = std::fs::read(&root_path).expect("read seeded root config");

        let profile_path = home_dir
            .path()
            .join("profiles")
            .join("mcp-isolation-profile")
            .join("config.yaml");

        let result = commit_mcp_server(
            ConfigScope::Profile("mcp-isolation-profile".to_string()),
            // CR-01: server names must round-trip through
            // sanitize_server_name — underscore, not hyphen (this test is
            // about profile isolation, not name validation).
            isolation_draft("isolation_server"),
        )
        .await;

        let root_after = std::fs::read(&root_path).expect("read root config after profile commit");

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        result.expect("profile-scoped commit must succeed with the root gate open");
        assert!(
            profile_path.exists(),
            "profile-scoped commit must create profiles/<name>/config.yaml"
        );
        let profile_after = ironhermes_core::config::Config::load_from(&profile_path)
            .expect("saved profile config must parse");
        assert!(
            profile_after.mcp_servers.contains_key("isolation_server"),
            "the profile's own config.yaml must carry the committed server"
        );
        assert_eq!(
            root_before, root_after,
            "a profile-scoped MCP commit must never change the root config.yaml bytes"
        );
    }

    // =====================================================================
    // CR-01 regression: `commit_mcp_server` name validation at the write
    // boundary (round-trip sanitize check + collision-with-a-different-
    // existing-server check). See `mcp_admin_api.rs::commit_mcp_server`'s
    // CR-01 comments.
    // =====================================================================

    /// (a) A name containing characters outside `[A-Za-z0-9_]` (the same
    /// round-trip check `parse_one_entry` already performs on the paste
    /// path) must be rejected at the write boundary too — nothing ever
    /// touches disk.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn commit_mcp_server_rejects_an_unsanitized_name() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let profile_path = home_dir
            .path()
            .join("profiles")
            .join("cr01-unsanitized-profile")
            .join("config.yaml");

        let result = commit_mcp_server(
            ConfigScope::Profile("cr01-unsanitized-profile".to_string()),
            isolation_draft("my server"),
        )
        .await;

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "an unsanitized server name must be rejected");
        assert!(
            !profile_path.exists(),
            "a rejected commit must never create the profile's config.yaml"
        );
    }

    /// (b) A raw name whose sanitized form collides with a DIFFERENT
    /// already-configured server's sanitized form must be rejected — even
    /// though the new name itself is already sanitized. The colliding
    /// existing entry is seeded directly (bypassing the API) to model a
    /// legacy/pre-fix name already on disk, exactly the scenario
    /// `mcp_group_server_key`'s ambiguous `HashMap::keys().find()` lookup
    /// could otherwise resolve to either server.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn commit_mcp_server_rejects_a_collision_with_a_different_existing_server() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        // The write gate is always read from a FRESH ROOT config regardless
        // of scope (module doc) — seed it open so the collision rejection
        // below is the actual reason the commit fails, not a closed gate.
        let root_cfg = ironhermes_core::config::Config {
            security: ironhermes_core::config::SecurityConfig {
                redact_secrets: true,
                web_config_write_enabled: true,
                web_process_control_enabled: false,
            },
            ..Default::default()
        };
        root_cfg.save().expect("seed root config.yaml");

        let profile_path = home_dir
            .path()
            .join("profiles")
            .join("cr01-collision-profile")
            .join("config.yaml");
        std::fs::create_dir_all(profile_path.parent().unwrap()).expect("mkdir profile dir");

        let mut seeded = ironhermes_core::config::Config::default();
        write_server_into(&mut seeded, "my-server", &draft_to_server_config(&isolation_draft("my-server")))
            .expect("seed write must succeed");
        seeded.save_to(&profile_path).expect("seed profile config.yaml");
        let before = std::fs::read(&profile_path).expect("read seeded profile config");

        let result = commit_mcp_server(
            ConfigScope::Profile("cr01-collision-profile".to_string()),
            isolation_draft("my_server"),
        )
        .await;

        let after = std::fs::read(&profile_path).expect("read profile config after rejected commit");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(
            result.is_err(),
            "a name colliding with a different existing server's sanitized form must be rejected"
        );
        assert_eq!(
            before, after,
            "a rejected collision commit must leave the on-disk config byte-identical"
        );
    }

    /// (c) Re-committing the SAME name (an update to an already-configured
    /// server) must still succeed — the collision check must not treat a
    /// server colliding with itself as an error.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn commit_mcp_server_allows_exact_same_name_recommit() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let root_cfg = ironhermes_core::config::Config {
            security: ironhermes_core::config::SecurityConfig {
                redact_secrets: true,
                web_config_write_enabled: true,
                web_process_control_enabled: false,
            },
            ..Default::default()
        };
        root_cfg.save().expect("seed root config.yaml");

        let profile_path = home_dir
            .path()
            .join("profiles")
            .join("cr01-recommit-profile")
            .join("config.yaml");
        std::fs::create_dir_all(profile_path.parent().unwrap()).expect("mkdir profile dir");

        let mut seeded = ironhermes_core::config::Config::default();
        write_server_into(&mut seeded, "my_server", &draft_to_server_config(&isolation_draft("my_server")))
            .expect("seed write must succeed");
        seeded.save_to(&profile_path).expect("seed profile config.yaml");

        let mut updated_draft = isolation_draft("my_server");
        updated_draft.timeout = 999;

        let result = commit_mcp_server(
            ConfigScope::Profile("cr01-recommit-profile".to_string()),
            updated_draft,
        )
        .await;

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        result.expect("re-committing the same server name must still succeed");
        let reloaded =
            ironhermes_core::config::Config::load_from(&profile_path).expect("reloaded config must parse");
        let raw = reloaded.mcp_servers.get("my_server").expect("server must still be present");
        let cfg = server_config_from_value(raw).expect("stored server must parse");
        assert_eq!(cfg.timeout, 999, "re-commit must apply the updated field");
    }

    /// `reload_mcp_and_report` (the D-12 live-apply helper) is a confirmed
    /// no-op for `ConfigScope::Profile` — returns `None` without touching
    /// any live manager, since a profile agent reads its config only at
    /// process launch.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn reload_mcp_and_report_is_a_no_op_for_profile_scope() {
        // No IRONHERMES_HOME mutation in this test (no config I/O) — the
        // lock is still taken first per this task's blanket discipline
        // (module doc: every test added by Task 2 holds ENV_LOCK).
        let _g = crate::server::test_support::env_lock();
        let config = ironhermes_core::config::Config::default();
        let result = reload_mcp_and_report(
            &ConfigScope::Profile("mcp-isolation-profile".to_string()),
            &config,
        )
        .await;
        assert!(
            result.is_none(),
            "the live-apply helper must return None (no reload attempted) for a profile scope"
        );
    }

    /// D-12 static-source invariant (GAP-1, gsd-nyquist-auditor 48.2): every
    /// PRODUCTION `save_scoped(` call site in this file must be followed
    /// within a small line window by a `reload_mcp_and_report(` call, so a
    /// future write path can never ship a silently-stale running agent.
    /// Mirrors the `include_str!` static scan pattern already established in
    /// this file (`task3_behavior_7_probe_machinery_never_touches_a_tool_registry`)
    /// and in `ironhermes-mcp/src/manager.rs`.
    ///
    /// Deliberately NOT a bare count-equality check: `complete_oauth_from_callback`
    /// has a legitimate extra `reload_mcp_and_report` call (line ~1010) with
    /// no paired `save_scoped` (it reloads after an OAuth token exchange, not
    /// a config write). So the invariant asserted here is "every save is
    /// followed by a reload nearby", not "the counts match".
    #[test]
    fn every_save_scoped_call_is_followed_by_reload_mcp_and_report_d12() {
        let src = include_str!("mcp_admin_api.rs");

        // Cut at the `mod tests {` line so test-code save_scoped/reload
        // calls (this module has its own fixtures) can never satisfy or
        // defeat the production-only scan.
        let mod_tests_idx = src
            .lines()
            .scan(0usize, |offset, line| {
                let start = *offset;
                *offset += line.len() + 1; // +1 for the newline
                Some((start, line))
            })
            .find(|(_, line)| {
                line.trim_start() == "mod tests {" || line.trim_end().ends_with("mod tests {")
            })
            .map(|(start, _)| start)
            .expect("mod tests { boundary must exist");
        let production_src = &src[..mod_tests_idx];

        // Strip comment-only lines so doc comments describing the write
        // contract cannot satisfy or defeat this scan.
        let code_only: String = production_src
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        // Small, explicit window: a `save_scoped(` call must be followed by
        // a `reload_mcp_and_report(` call within the next 5 lines (covers
        // the observed 1-line gap at every current call site, with slack
        // for a wrapped/reformatted call).
        const WINDOW_LINES: usize = 5;

        let lines: Vec<&str> = code_only.lines().collect();
        let mut unpaired: Vec<usize> = Vec::new();
        for (idx, line) in lines.iter().enumerate() {
            // Exclude the definition site itself (`fn save_scoped(`).
            if line.contains("save_scoped(") && !line.contains("fn save_scoped(") {
                let window_end = (idx + 1 + WINDOW_LINES).min(lines.len());
                let found = lines[idx + 1..window_end]
                    .iter()
                    .any(|l| l.contains("reload_mcp_and_report("));
                if !found {
                    unpaired.push(idx + 1); // 1-based line number in the scanned slice
                }
            }
        }

        assert!(
            unpaired.is_empty(),
            "D-12 violation: production `save_scoped(` call(s) at scanned-source line(s) {unpaired:?} \
             have no `reload_mcp_and_report(` call within {WINDOW_LINES} lines — a write path was \
             added without a matching live-apply call, which would ship a silently-stale running agent"
        );
    }
}

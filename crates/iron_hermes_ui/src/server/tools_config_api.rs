//! Phase 48.2 Plan 01 (D-05/D-08/D-10/D-11/D-12/D-13/D-14/D-16/D-20): the
//! Tools page's config-editing server surface — reads the UNFILTERED tool
//! catalog ([`ironhermes_tools::registry::ToolRegistry::catalog_rows`], never
//! the LLM-facing `get_definitions()` view, RESEARCH.md Pitfall 1) and writes
//! the `tools:` section of `config.yaml` through the same gated,
//! `ConfigScope`-parameterized path every write in this phase follows:
//! validate (no disk I/O) -> [`resolve_scope_target`] (fresh disk read) ->
//! [`check_tools_write_gate`] -> mutate -> [`save_scoped`] -> live apply.
//!
//! # `ConfigScope` — root vs profile, never conflated
//!
//! `Root` writes always go through `Config::save()` (the hardcoded root
//! path); `Profile(name)` writes always go through
//! `Config::save_to(profile_dir_for(name).join("config.yaml"))` — mirrors
//! `profile_api.rs::update_profile_config_impl`'s "never writes to
//! `get_hermes_home().join(\"config.yaml\")`" doc comment (D-08).
//!
//! # The write gate is always read from ROOT
//!
//! [`check_tools_write_gate`] and the page-state gate check both read
//! `security.web_config_write_enabled` from a **fresh root** `Config::load()`
//! regardless of which scope is being edited. This is deliberate: the gate is
//! the operator's policy for the web surface itself, not a property of the
//! config file being written. A profile with no `config.yaml` yet resolves to
//! `Default::default()`, whose `web_config_write_enabled` is `false` — reading
//! the gate from a profile's own (possibly-absent) config would lock profile
//! editing permanently.
//!
//! # Test-reachability discipline (G-50.2-2b precedent, `state.rs:269-280`)
//!
//! In-file `#[cfg(test)]` modules never call `install_global_app_state()`, so
//! `global_app_state()` panics if a test-reachable path calls it. Every impl
//! fn in this file that needs the live [`ironhermes_tools::registry::ToolRegistry`]
//! (known toolset/tool names for validation, live-apply) takes that data as an
//! explicit parameter or reads it via [`crate::server::state::try_global_app_state`]
//! (no-op when uninitialized) — never `global_app_state()` directly from a
//! path a unit test exercises. This keeps the pure config-mutation core
//! (`build_tools_page_state`, `validate_toolset_target`, `apply_toolset_toggle`,
//! `resolve_scope_target`, `check_tools_write_gate`, `save_scoped`,
//! `mcp_group_server_key`) fully unit-testable without a running server.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

// =============================================================================
// DTOs — shared shape on both the wasm client and the native server.
// =============================================================================

/// Which config.yaml a write targets (D-08). `Root` is the default — every
/// caller that does not explicitly select a profile edits the operator's own
/// `~/.ironhermes/config.yaml`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub enum ConfigScope {
    #[default]
    Root,
    Profile(String),
}

/// One unmet prerequisite, stripped down to display-safe fields — never
/// carries a credential value (T-48.2-01-04).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MissingPrereq {
    pub kind: String,
    pub name: String,
    pub description: String,
}

/// D-16: a tool's availability, surfaced honestly — `Unknown` is a distinct
/// state from `Unavailable` so a failed check is never rendered as either a
/// false "available" or a false "unavailable" claim.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ToolAvailability {
    Available,
    Unavailable { missing: Vec<MissingPrereq> },
    Unknown { reason: String },
}

/// One card's worth of catalog data. `disabled_override` mirrors membership
/// in `config.tools.disabled` — independent of the toolset's own enabled
/// state, so a card can be `disabled_override: true` inside an enabled
/// toolset, or simply unavailable inside a disabled one; the unfiltered read
/// (D-16) means every combination reaches the browser.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ToolCatalogEntry {
    pub name: String,
    pub description: String,
    pub toolset: String,
    pub group: String,
    pub disabled_override: bool,
    pub availability: ToolAvailability,
    /// D-20 (Phase 48.2 Plan 10/G-48.2-4): this card's origin. A tool cannot
    /// be built-in inside an MCP group or vice versa — a card's origin is its
    /// GROUP's origin by construction, so this field is a PROJECTION of the
    /// single derivation `resolve_group_kind_and_enabled` already performs,
    /// stamped onto each member card so `ToolCard` can render provenance
    /// without needing its parent `ToolsetGroup` in scope. There is exactly
    /// one call to `resolve_group_kind_and_enabled` per group; this and the
    /// group's own `kind` field both read that one result.
    pub origin: ToolsetKind,
    /// Phase 48.2 Plan 11 (G-48.2-6 slice a): the stable identifier of a
    /// separate process that executes this tool's effects — projected
    /// unchanged from [`ironhermes_tools::registry::ToolCatalogRow::runtime_dependency`].
    /// `Some(GATEWAY_RUNTIME_DEPENDENCY)` for `cronjob`; `None` for a tool
    /// with no such dependency. Paired client-side with the live
    /// `GatewayRuntimeStatus` (`gateway_status_api.rs`) by `ToolCard`'s
    /// annotation predicate — this field alone never implies the gateway is
    /// down, only that the tool has something to lose if it is.
    pub runtime_dependency: Option<String>,
}

/// Portable duplicate of
/// `ironhermes_tools::registry::GATEWAY_RUNTIME_DEPENDENCY` — that crate is
/// a native-only dependency of this crate (Cargo.toml
/// `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`), so the
/// wasm-compiled `ToolCard` annotation predicate needs its own portable copy
/// of the identifier string, following the `CANONICAL_TOOL_CREDENTIAL_KEYS`
/// precedent (`tools_credentials_api.rs`) verbatim. A native-only test below
/// asserts this stays byte-identical to the source of truth so the two
/// constants cannot silently drift.
#[allow(dead_code)] // consumed by the drift test below; unlike CANONICAL_TOOL_CREDENTIAL_KEYS, no render call site names this constant directly (ToolCard compares the DTO's own String field instead) — the bin-crate dead-code lint cannot see the test-only usage from outside `#[cfg(test)]`
pub const GATEWAY_RUNTIME_DEPENDENCY: &str = "gateway";

/// D-20: a display group's origin. `BuiltIn` groups are writable via
/// `toggle_toolset` (`tools.toolsets.<name>`); `Mcp` groups are NOT — their
/// master toggle is the `mcp_servers.<name>.enabled` server-lifecycle field,
/// owned by later plans in this phase.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ToolsetKind {
    BuiltIn,
    Mcp { server: String },
}

/// One display-group section of the Tools page (D-14/D-20): a toolset header
/// (master toggle + enabled state) plus its member tool cards.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ToolsetGroup {
    pub name: String,
    pub kind: ToolsetKind,
    pub enabled: bool,
    pub tools: Vec<ToolCatalogEntry>,
}

/// The full Tools page state for one scope: the write-gate state, the scope
/// being viewed, and every display group with its cards. `gate_open` is
/// ALWAYS read from the root config (see module doc) — never from
/// `scope`-resolved config, even when `scope` is a profile.
///
/// `bulk_targets` (Phase 48.2 Plan 06 Task 2/D-17) is computed server-side
/// by [`bulk_targets`] and shipped as PART of this already-fetched state —
/// `ironhermes-core` (home of `ALL_TOOLSETS`) is a
/// `cfg(not(target_arch = "wasm32"))`-only dependency of this crate, so the
/// Task 3 confirm dialog cannot call the `bulk_targets` fn client-side.
/// Reading this field instead satisfies the same "no fetch backs the
/// confirm" requirement — it is data already on the page, just computed on
/// the server rather than in wasm.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ToolsPageState {
    pub gate_open: bool,
    pub scope: ConfigScope,
    pub scope_label: String,
    pub toolsets: Vec<ToolsetGroup>,
    pub bulk_targets: Vec<String>,
}

// =============================================================================
// Phase 48.2 Plan 06 Task 2 (D-17): bulk ENABLE ALL / DISABLE ALL — a
// single gated atomic write over the built-in toolset taxonomy, excluding
// MCP display groups and both sentinel toolsets by construction (REVIEWS
// finding 8). `HIGH_BLAST_RADIUS_TOOLSETS` is portable (a plain `&[&str]`
// literal) so the Task 3 confirm dialog can read it client-side. `bulk_targets`
// itself is `cfg(not(target_arch = "wasm32"))`-only — `ironhermes-core`
// (home of `ALL_TOOLSETS`) is a server-only dependency of this crate — so
// its result is instead shipped to the client as `ToolsPageState::bulk_targets`,
// computed once per fetch and reused with no extra round trip.
// =============================================================================

/// D-17: the three default-off, high-blast-radius toolsets that ENABLE ALL
/// turns on. Named explicitly in the confirm copy so an operator sees
/// exactly what a blanket enable would activate. A test asserts every name
/// here is present in the core crate's own default toolsets map AND ships
/// disabled there, so this constant cannot silently drift from
/// `ironhermes_core::config::ToolsConfig::default()`.
#[allow(dead_code)] // consumed by Task 3's bulk_confirm.rs ENABLE ALL copy; also asserted directly by this file's own test
pub const HIGH_BLAST_RADIUS_TOOLSETS: &[&str] = &["browser", "code", "web"];

/// Derive the bulk-operation target list from the built-in taxonomy, not by
/// excluding prefixes (REVIEWS finding 8). Returns every group whose `kind`
/// is `ToolsetKind::BuiltIn` AND whose name is present in
/// `ironhermes_core::constants::ALL_TOOLSETS`, sorted and de-duplicated.
/// Both conditions are load-bearing: `BuiltIn` drops MCP display groups
/// (`mcp__<server>`, D-17's "MCP servers are not affected"); `ALL_TOOLSETS`
/// membership drops the flat `"mcp"` and `"kanban"` sentinel toolsets, which
/// the registry exempts from `is_toolset_enabled` — writing either key would
/// persist an inert entry nothing reads (T-48.2-06-07).
///
/// `cfg(not(target_arch = "wasm32"))`-only: `ironhermes-core` is a
/// server-only dependency of this crate (Cargo.toml
/// `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`). The client
/// reads the result via [`ToolsPageState::bulk_targets`] instead — computed
/// here, shipped as part of the already-fetched page state.
#[cfg(not(target_arch = "wasm32"))]
pub fn bulk_targets(groups: &[ToolsetGroup]) -> Vec<String> {
    let mut targets: Vec<String> = groups
        .iter()
        .filter(|g| matches!(g.kind, ToolsetKind::BuiltIn))
        .filter(|g| ironhermes_core::constants::ALL_TOOLSETS.contains(&g.name.as_str()))
        .map(|g| g.name.clone())
        .collect();
    targets.sort();
    targets.dedup();
    targets
}

/// One toolset's bulk-flip outcome — `changed` distinguishes "already in
/// the requested state" from "actually flipped", so the caller can report
/// exactly what happened rather than assuming every target changed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BulkToggleOutcome {
    pub toolset: String,
    pub enabled: bool,
    pub changed: bool,
}

/// Pure(-ish) core of [`set_toolsets_bulk`] — `groups` is injected (the
/// caller's own live-catalog-derived `ToolsetGroup` list) rather than read
/// from the registry, so this is directly unit-testable with hand-built
/// groups. Order mirrors every other write path in this file: fresh
/// scope-resolved disk read -> fail-closed write gate -> mutate -> single
/// atomic save -> live apply. `bulk_targets` computes the target list from
/// `groups` server-side — never a client-supplied list (T-48.2-06-03).
#[cfg(not(target_arch = "wasm32"))]
async fn set_toolsets_bulk_with_groups(
    scope: ConfigScope,
    enable: bool,
    groups: &[ToolsetGroup],
) -> Result<Vec<BulkToggleOutcome>, String> {
    // Step 1 — fresh, scope-resolved disk read (never the startup snapshot).
    let (mut config, target) = resolve_scope_target(&scope)?;

    // Step 2 — fail-closed write gate.
    check_tools_write_gate()?;

    // Step 3 — target list, server-side, from ALL_TOOLSETS ∩ the catalog.
    let targets = bulk_targets(groups);

    // Step 4 — idempotent set per target, one atomic save for the whole
    // bulk operation (not one write per toolset) — a failure therefore
    // leaves nothing applied.
    let outcomes: Vec<BulkToggleOutcome> = targets
        .into_iter()
        .map(|name| {
            let before = config.tools.is_toolset_enabled(&name);
            apply_toolset_toggle(&mut config, &name, enable);
            BulkToggleOutcome {
                toolset: name,
                enabled: enable,
                changed: before != enable,
            }
        })
        .collect();
    save_scoped(&config, &target)?;

    // Step 5 — live apply (D-12): the running registry sees the change in
    // this same request.
    apply_live_toolset_config(&scope, &config).await;

    Ok(outcomes)
}

/// Bulk ENABLE ALL / DISABLE ALL (D-17). Computes its own target list
/// server-side from the live registry catalog rather than trusting a list
/// sent by the client (T-48.2-06-03); resolves the scope for a fresh
/// on-disk read; checks the write gate; applies the requested state to
/// every target as an idempotent set; saves once through `save_scoped`; and
/// returns one outcome per target.
#[server]
pub async fn set_toolsets_bulk(
    scope: ConfigScope,
    enable: bool,
) -> Result<Vec<BulkToggleOutcome>, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let rows = live_catalog_rows().await;
        let (config, _target) = resolve_scope_target(&scope).map_err(ServerFnError::new)?;
        let gate_open = root_gate_open();
        let state = build_tools_page_state(rows, &config, gate_open, scope.clone());
        set_toolsets_bulk_with_groups(scope, enable, &state.toolsets)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, enable);
        unreachable!("server fn body never runs on the wasm client")
    }
}

// =============================================================================
// Server-only helpers — pure where possible, so tests never need a running
// server or an installed global AppState (see module doc).
// =============================================================================

/// D-08: resolve `scope` to a fresh on-disk `Config` plus the path a save
/// must target (`None` = root's hardcoded path, `Some(path)` = a profile's
/// `config.yaml`). Never the startup config snapshot — always a fresh read.
///
/// Phase 48.2 Plan 12: promoted from module-private to `pub(crate)` — the
/// crate's SHARED scope-resolution helper. `mcp_admin_api.rs` still carries
/// its own duplicate pair (module doc there calls it a "D-08 sibling");
/// converging that duplicate onto this promoted pair is a follow-up this
/// plan deliberately leaves in place (`files_modified` does not include
/// `mcp_admin_api.rs` — see 48.2-12-SUMMARY.md). `platform_config_api.rs`
/// is the first NEW caller of the promoted pair, so at least the second
/// copy stops here.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn resolve_scope_target(
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

/// D-10: fail-closed write gate — reads `security.web_config_write_enabled`
/// from a FRESH ROOT `Config::load()` regardless of the scope being edited
/// (module doc explains why). Returns the exact `"Config writes are
/// disabled"` error string every other write-side `#[server]` fn in this
/// crate already uses (`api.rs::update_voice_config`).
#[cfg(not(target_arch = "wasm32"))]
fn check_tools_write_gate() -> Result<(), String> {
    let root_config = ironhermes_core::config::Config::load()
        .map_err(|e| format!("Config load failed: {e}"))?;
    if !root_config.security.web_config_write_enabled {
        return Err("Config writes are disabled".to_string());
    }
    Ok(())
}

/// The page-state read-side twin of [`check_tools_write_gate`]: a boolean,
/// fail-closed on any load error, never an `Err` — the page must still
/// render (read-only) when the root config cannot be read.
#[cfg(not(target_arch = "wasm32"))]
fn root_gate_open() -> bool {
    ironhermes_core::config::Config::load()
        .map(|c| c.security.web_config_write_enabled)
        .unwrap_or(false)
}

/// D-13: atomic write — `Config::save()` when `target` is `None` (root),
/// `Config::save_to(path)` otherwise (profile). Never both, never the root
/// path for a profile.
///
/// Phase 48.2 Plan 12: promoted from module-private to `pub(crate)` — the
/// crate's SHARED atomic-save helper, paired with [`resolve_scope_target`]
/// (see that fn's doc comment for the promotion rationale).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn save_scoped(
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

/// D-20: recover the `mcp_servers` config key whose sanitized form produced
/// `group` (an `mcp__<sanitized>` display group name). Sanitization runs
/// server-side only. Returns `None` when no configured server's sanitized
/// name matches (e.g. a server removed from config while its tools are still
/// registered) — the caller falls back to the raw suffix and marks the group
/// `enabled: false`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn mcp_group_server_key(
    group: &str,
    mcp_servers: &std::collections::HashMap<String, serde_yaml::Value>,
) -> Option<String> {
    mcp_servers
        .keys()
        .find(|k| format!("mcp__{}", ironhermes_mcp::sanitize_server_name(k)) == group)
        .cloned()
}

/// D-20: resolve a display group's `ToolsetKind` and live `enabled` state.
/// A group NOT prefixed `mcp__` is `BuiltIn`, gated by
/// `ToolsConfig::is_toolset_enabled` — `is_toolset_enabled` is NEVER
/// consulted for an `mcp__` group (the registry exempts `"mcp"` from that
/// filter, so its answer would be a fiction for MCP groups).
#[cfg(not(target_arch = "wasm32"))]
fn resolve_group_kind_and_enabled(
    group: &str,
    config: &ironhermes_core::config::Config,
) -> (ToolsetKind, bool) {
    if let Some(suffix) = group.strip_prefix("mcp__") {
        match mcp_group_server_key(group, &config.mcp_servers) {
            Some(server_key) => {
                // McpServerConfig::enabled defaults to true (config.rs) —
                // match that default when the raw Value has no explicit key.
                let enabled = config
                    .mcp_servers
                    .get(&server_key)
                    .and_then(|v| v.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                (ToolsetKind::Mcp { server: server_key }, enabled)
            }
            None => (
                ToolsetKind::Mcp {
                    server: suffix.to_string(),
                },
                false,
            ),
        }
    } else {
        (ToolsetKind::BuiltIn, config.tools.is_toolset_enabled(group))
    }
}

/// Pure core of [`get_tools_page_state`]: turns an unfiltered row set plus a
/// resolved `Config` into the full page DTO. No disk I/O, no global state —
/// directly unit-testable with hand-built `ToolCatalogRow` values.
#[cfg(not(target_arch = "wasm32"))]
fn build_tools_page_state(
    rows: Vec<ironhermes_tools::registry::ToolCatalogRow>,
    config: &ironhermes_core::config::Config,
    gate_open: bool,
    scope: ConfigScope,
) -> ToolsPageState {
    let scope_label = match &scope {
        ConfigScope::Root => "ROOT".to_string(),
        ConfigScope::Profile(name) => name.clone(),
    };

    let mut groups_map: std::collections::BTreeMap<String, Vec<ToolCatalogEntry>> =
        std::collections::BTreeMap::new();
    for row in rows {
        let disabled_override = config.tools.disabled.iter().any(|d| d == &row.name);
        let availability = if row.available {
            ToolAvailability::Available
        } else {
            let missing = row
                .missing_prerequisites
                .iter()
                .map(|p| MissingPrereq {
                    kind: p.kind.clone(),
                    name: p.name.clone(),
                    description: p.description.clone(),
                })
                .collect();
            ToolAvailability::Unavailable { missing }
        };
        let runtime_dependency = row.runtime_dependency.map(|s| s.to_string());
        groups_map
            .entry(row.group.clone())
            .or_default()
            .push(ToolCatalogEntry {
                name: row.name,
                description: row.description,
                toolset: row.toolset,
                group: row.group,
                disabled_override,
                availability,
                // Placeholder — overwritten below once the group's kind is
                // resolved. Kept as a real, valid `ToolsetKind` value (never
                // left uninitialized) so this struct literal type-checks
                // standalone; the second pass is the single source of truth
                // for what every entry's `origin` actually ends up as.
                origin: ToolsetKind::BuiltIn,
                runtime_dependency,
            });
    }

    let toolsets: Vec<ToolsetGroup> = groups_map
        .into_iter()
        .map(|(group_name, mut tools)| {
            // One call per group (D-20 discipline) — its result backs both
            // the group's own `kind` and every member card's `origin`.
            let (kind, enabled) = resolve_group_kind_and_enabled(&group_name, config);
            for tool in &mut tools {
                tool.origin = kind.clone();
            }
            ToolsetGroup {
                name: group_name,
                kind,
                enabled,
                tools,
            }
        })
        .collect();

    // Phase 48.2 Plan 06 Task 2 (D-17): computed here (native-only —
    // `bulk_targets` needs `ironhermes_core::constants::ALL_TOOLSETS`) and
    // shipped as part of this already-fetched state so the Task 3 confirm
    // dialog never needs its own fetch.
    let bulk_targets = bulk_targets(&toolsets);

    ToolsPageState {
        gate_open,
        scope,
        scope_label,
        toolsets,
        bulk_targets,
    }
}

/// Read the live, unfiltered catalog for the currently-installed
/// `AppState`. `None` when `AppState` is not installed (test-reachability
/// discipline, module doc) — callers treat that as "no rows" rather than
/// panicking.
#[cfg(not(target_arch = "wasm32"))]
async fn live_catalog_rows() -> Vec<ironhermes_tools::registry::ToolCatalogRow> {
    // 49.4-02 fix (folded todo 2026-08-28): `crate::server::state` itself
    // stays `feature = "server"`-gated (it needs `tokio::sync::RwLock`,
    // only available via `tokio/full`, which only the `server` feature
    // enables) — unlike `profile_api`'s pure helpers, it cannot be widened
    // to `not(target_arch = "wasm32")`. This fn's own gate is (correctly)
    // just `not(target_arch = "wasm32")` since ALL its callers assume that
    // much, so the `state` access is split internally instead: when
    // `server` is off (e.g. `cargo test --workspace`, which never requests
    // it for this non-default-member crate), behave exactly like the
    // already-handled "no AppState installed" `None` arm below.
    #[cfg(feature = "server")]
    {
        match crate::server::state::try_global_app_state() {
            Some(state) => state.runtime.registry().read().await.catalog_rows(),
            None => Vec::new(),
        }
    }
    #[cfg(not(feature = "server"))]
    Vec::new()
}

/// D-12: apply a saved toolset config to the LIVE, in-process registry so
/// the running agent sees the change in the SAME request. A no-op for
/// `ConfigScope::Profile` — profile agents (kanban workers, bot-mode
/// subprocesses) read their config at process launch, so there is no live
/// registry to update here. Uses [`crate::server::state::try_global_app_state`]
/// (never the panicking `global_app_state()`) so this is also safe to call
/// from a Root-scope success-path unit test with no AppState installed.
#[cfg(not(target_arch = "wasm32"))]
async fn apply_live_toolset_config(scope: &ConfigScope, config: &ironhermes_core::config::Config) {
    if !matches!(scope, ConfigScope::Root) {
        return;
    }
    // 49.4-02 fix: same `state` split rationale as `live_catalog_rows`
    // above — without `server`, behave like the already-handled "no
    // AppState installed" no-op.
    #[cfg(feature = "server")]
    if let Some(state) = crate::server::state::try_global_app_state() {
        let mut guard = state.runtime.registry().write().await;
        guard.set_toolset_config(Some(config.tools.clone()));
    }
    #[cfg(not(feature = "server"))]
    let _ = config;
}

/// T-48.2-01-03: validate a toolset TARGET name before any disk I/O.
/// Rejects an `mcp__`-prefixed group with a fixed, actionable error (writing
/// `tools.toolsets.mcp__<name>` would persist a key the registry never
/// reads — a silent no-op, rejected under 47.3 D-03) BEFORE checking
/// membership in `known_groups`, so an MCP group name that IS a known
/// display group still gets the specific MCP error, not a generic "unknown
/// toolset".
#[cfg(not(target_arch = "wasm32"))]
fn validate_toolset_target(
    name: &str,
    known_groups: &std::collections::HashSet<String>,
) -> Result<(), String> {
    if name.starts_with("mcp__") {
        return Err(
            "MCP server groups are toggled through the MCP SERVERS section, not tools.toolsets"
                .to_string(),
        );
    }
    if !known_groups.contains(name) {
        return Err(format!("unknown toolset: {name}"));
    }
    Ok(())
}

/// Idempotent set, keyed on `enable` — a double click cannot desynchronize
/// the UI from disk (a second identical toggle call is a no-op write, not a
/// blind flip).
#[cfg(not(target_arch = "wasm32"))]
fn apply_toolset_toggle(config: &mut ironhermes_core::config::Config, name: &str, enable: bool) {
    config.tools.toolsets.insert(
        name.to_string(),
        ironhermes_core::config::ToolsetEntry { enabled: enable },
    );
}

/// Pure(-ish) core of [`toggle_toolset`] — `known_groups` is injected rather
/// than read from the live registry, so this is directly unit-testable.
/// Live-apply is safe to call unconditionally (see
/// [`apply_live_toolset_config`]'s doc comment on `try_global_app_state`).
#[cfg(not(target_arch = "wasm32"))]
async fn toggle_toolset_with_known_groups(
    scope: ConfigScope,
    name: String,
    enable: bool,
    known_groups: &std::collections::HashSet<String>,
) -> Result<(), String> {
    // Step 1 — validate BEFORE any disk I/O.
    validate_toolset_target(&name, known_groups)?;

    // Step 2 — fresh, scope-resolved disk read (never the startup snapshot).
    let (mut config, target) = resolve_scope_target(&scope)?;

    // Step 3 — fail-closed write gate.
    check_tools_write_gate()?;

    // Step 4 — mutate + atomic save.
    apply_toolset_toggle(&mut config, &name, enable);
    save_scoped(&config, &target)?;

    // Step 5 — live apply (D-12): the running registry sees the change in
    // this same request.
    apply_live_toolset_config(&scope, &config).await;

    Ok(())
}

/// T-48.2-01-03 sibling for per-tool names: validate a tool name against
/// `known_tools` before any disk I/O (Behavior 5).
#[cfg(not(target_arch = "wasm32"))]
fn validate_known_tool(
    name: &str,
    known_tools: &std::collections::HashSet<String>,
) -> Result<(), String> {
    if !known_tools.contains(name) {
        return Err(format!("unknown tool: {name}"));
    }
    Ok(())
}

/// Idempotent set operation on `config.tools.disabled`, keyed on `disable`
/// (Behavior 2/3): inserting an already-present name is a no-op (no
/// duplicate); removing an absent name is a no-op. The list is sorted
/// before saving so repeated writes produce a stable file.
#[cfg(not(target_arch = "wasm32"))]
fn apply_tool_disabled_toggle(
    config: &mut ironhermes_core::config::Config,
    tool: &str,
    disable: bool,
) {
    if disable {
        if !config.tools.disabled.iter().any(|d| d == tool) {
            config.tools.disabled.push(tool.to_string());
        }
    } else {
        config.tools.disabled.retain(|d| d != tool);
    }
    config.tools.disabled.sort();
}

/// Pure(-ish) core of [`toggle_tool_disabled`] — symmetric with
/// [`toggle_toolset_with_known_groups`]: `known_tools` is injected rather
/// than read from the live registry, so this is directly unit-testable.
#[cfg(not(target_arch = "wasm32"))]
async fn toggle_tool_disabled_with_known_tools(
    scope: ConfigScope,
    tool: String,
    disable: bool,
    known_tools: &std::collections::HashSet<String>,
) -> Result<(), String> {
    // Step 1 — validate BEFORE any disk I/O.
    validate_known_tool(&tool, known_tools)?;

    // Step 2 — fresh, scope-resolved disk read (never the startup snapshot).
    let (mut config, target) = resolve_scope_target(&scope)?;

    // Step 3 — fail-closed write gate.
    check_tools_write_gate()?;

    // Step 4 — mutate + atomic save.
    apply_tool_disabled_toggle(&mut config, &tool, disable);
    save_scoped(&config, &target)?;

    // Step 5 — live apply (D-12).
    apply_live_toolset_config(&scope, &config).await;

    Ok(())
}

// =============================================================================
// #[server] fns — thin wrappers over the impl fns above (dioxus fullstack
// codec split: real body compiles server-side only, client gets a network
// stub).
// =============================================================================

/// Read the unfiltered Tools page state for `scope` (D-16/D-20). The
/// catalog read applies NO toolset filter, NO availability filter, and NO
/// per-tool disable filter — a card the operator must fix has to reach the
/// browser.
#[server]
pub async fn get_tools_page_state(scope: ConfigScope) -> Result<ToolsPageState, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (config, _target) = resolve_scope_target(&scope).map_err(ServerFnError::new)?;
        let gate_open = root_gate_open();
        let rows = live_catalog_rows().await;
        Ok(build_tools_page_state(rows, &config, gate_open, scope))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scope;
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Instant-write toggle for a BUILT-IN toolset's master enable/disable
/// state (D-11/D-14). Rejects `mcp__`-prefixed groups (D-20 —
/// `mcp_servers.<name>.enabled` is the real MCP lifecycle switch, owned by
/// later plans).
#[server]
pub async fn toggle_toolset(
    scope: ConfigScope,
    name: String,
    enable: bool,
) -> Result<(), ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let known_groups: std::collections::HashSet<String> = live_catalog_rows()
            .await
            .into_iter()
            .map(|r| r.group)
            .collect();
        toggle_toolset_with_known_groups(scope, name, enable, &known_groups)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, name, enable);
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Instant-write toggle for a single tool's `tools.disabled` override
/// (D-11), symmetric with [`toggle_toolset`]: validate -> fresh scope read
/// -> gate -> idempotent mutate -> atomic save -> live apply.
#[server]
pub async fn toggle_tool_disabled(
    scope: ConfigScope,
    tool: String,
    disable: bool,
) -> Result<(), ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let known_tools: std::collections::HashSet<String> = live_catalog_rows()
            .await
            .into_iter()
            .map(|r| r.name)
            .collect();
        toggle_tool_disabled_with_known_tools(scope, tool, disable, &known_tools)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (scope, tool, disable);
        unreachable!("server fn body never runs on the wasm client")
    }
}

// =============================================================================
// Phase 48.2 Plan 03 (D-05/D-10/D-11/D-13/D-21): staged-write settings for
// the rest of `tools:` — the global timeout, per-tool timeout overrides, and
// the three web provider chains as first-class reorderable controls. Same
// gated, scope-aware, atomic write path Plan 01 established above:
// validate (no disk I/O) -> resolve_scope_target (fresh disk read) ->
// check_tools_write_gate (ROOT, always) -> mutate -> save_scoped -> live
// apply.
// =============================================================================

/// One per-tool timeout override row (D-06's signed disable-the-bound
/// convention). `Vec<TimeoutOverrideRow>` on the wire rather than a
/// `HashMap` so the staged UI can render an ordered, editable list.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TimeoutOverrideRow {
    pub tool: String,
    pub seconds: i64,
}

/// One entry in a provider chain, with its credential-presence answer.
/// Carries the credential's NAME only — never a value (T-48.2-03-02).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChainEntryView {
    pub provider: String,
    pub credential: Option<String>,
    pub skipped: bool,
}

/// One tool's provider chain plus its legal provider set. `legal_providers`
/// is read from the core crate's per-tool constant, never retyped in this
/// crate (plan prohibition — the core crate's chain validator stays the
/// single source of truth for the legal set).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChainView {
    pub tool: String,
    pub entries: Vec<ChainEntryView>,
    pub legal_providers: Vec<String>,
}

/// The staged-write payload for `update_tools_settings` — deliberately
/// touches nothing else in `tools:` (no toolsets/disabled/credentials
/// fields here; `merge_tools_settings` below mutates only these four).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ToolsSettingsPayload {
    pub timeout_secs: u64,
    pub timeout_overrides: Vec<TimeoutOverrideRow>,
    pub web_search: Vec<String>,
    pub web_answer: Vec<String>,
    pub web_extract: Vec<String>,
}

/// The read-side view for the staged settings panel. `gate_open` mirrors
/// `ToolsPageState::gate_open` so the panel can render read-only without a
/// second round trip.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ToolsSettingsView {
    pub gate_open: bool,
    pub timeout_secs: u64,
    pub timeout_overrides: Vec<TimeoutOverrideRow>,
    pub chains: Vec<ChainView>,
}

/// Canonical credential env var for a chain provider name, or `None` when
/// the provider needs no credential (`ddg`, `local`). A test iterates all
/// three per-tool legal provider constants and asserts every name is
/// handled here explicitly, so a future provider added to the core crate
/// cannot silently render without a credential answer.
pub fn credential_for_chain_provider(provider: &str) -> Option<&'static str> {
    match provider {
        "exa" => Some("EXA_API_KEY"),
        "brave" => Some("BRAVE_API_KEY"),
        "tavily" => Some("TAVILY_API_KEY"),
        "perplexity" => Some("PERPLEXITY_API_KEY"),
        "firecrawl" => Some("FIRECRAWL_API_KEY"),
        "ddg" | "local" => None,
        _ => None,
    }
}

/// Validate a staged settings payload with NO disk I/O (Step 1 of the
/// staged-write order). Concatenates every problem so an operator with two
/// typos across two chains sees both in one round trip. Chain legality is
/// delegated entirely to `ToolsConfig::validate_chains` — the per-tool legal
/// sets and the empty-chain rule stay defined in exactly one place (the core
/// crate).
#[cfg(not(target_arch = "wasm32"))]
fn validate_tools_settings(
    payload: &ToolsSettingsPayload,
    known_tool_names: &std::collections::HashSet<String>,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    if payload.timeout_secs == 0 {
        errors.push("timeout_secs must be greater than 0".to_string());
    }

    for row in &payload.timeout_overrides {
        if !known_tool_names.contains(&row.tool) {
            errors.push(format!(
                "unknown tool in timeout_overrides: {}",
                row.tool
            ));
        }
    }

    let candidate = ironhermes_core::config::ToolsConfig {
        web_search: ironhermes_core::config::WebToolChainConfig {
            chain: payload.web_search.clone(),
        },
        web_answer: ironhermes_core::config::WebToolChainConfig {
            chain: payload.web_answer.clone(),
        },
        web_extract: ironhermes_core::config::WebToolChainConfig {
            chain: payload.web_extract.clone(),
        },
        ..Default::default()
    };
    if let Err(chain_errors) = candidate.validate_chains() {
        errors.extend(chain_errors);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Mutate ONLY `timeout_secs`, `timeout_overrides` and the three chains —
/// touches no other field of `tools:` (`toolsets`, `disabled`,
/// `credentials`, `skip_prompts` all untouched).
#[cfg(not(target_arch = "wasm32"))]
fn merge_tools_settings(
    config: &mut ironhermes_core::config::Config,
    payload: &ToolsSettingsPayload,
) {
    config.tools.timeout_secs = payload.timeout_secs;
    config.tools.timeout_overrides = payload
        .timeout_overrides
        .iter()
        .map(|row| (row.tool.clone(), row.seconds))
        .collect();
    config.tools.web_search.chain = payload.web_search.clone();
    config.tools.web_answer.chain = payload.web_answer.clone();
    config.tools.web_extract.chain = payload.web_extract.clone();
}

/// Resolve the D-19 tool-credential snapshot for `config`, opening the
/// vault store only when the operator enabled it — mirrors
/// `app_runtime_factory.rs`'s production resolution shape exactly
/// (T-41.3-53: an operator who never turned the vault on can never be
/// stopped from rendering the page by it). Propagates a sealed/corrupt
/// vault's error loudly rather than silently rendering every entry as
/// skipped.
#[cfg(not(target_arch = "wasm32"))]
async fn resolve_tool_credentials(
    config: &ironhermes_core::config::Config,
) -> Result<ironhermes_tools::credentials::ToolCredentials, String> {
    let store = if config.vault.enabled {
        Some(
            ironhermes_vault::open_store(&ironhermes_core::resolve_vault_config(config))
                .map_err(|e| format!("failed to open the vault store: {e}"))?,
        )
    } else {
        None
    };
    ironhermes_tools::credentials::ToolCredentials::resolve(config, store.as_deref())
        .await
        .map_err(|e| format!("tool-credential resolution failed: {e}"))
}

/// Build one chain's `ChainView` from its config chain and legal-provider
/// set, resolving each entry's skipped state against `credentials`. An
/// entry whose provider needs no credential (`credential_for_chain_provider`
/// returns `None`) is never marked skipped.
#[cfg(not(target_arch = "wasm32"))]
fn build_chain_view(
    tool: &str,
    chain: &[String],
    legal: &[&str],
    credentials: &ironhermes_tools::credentials::ToolCredentials,
) -> ChainView {
    let entries = chain
        .iter()
        .map(|provider| {
            let credential = credential_for_chain_provider(provider);
            let skipped = credential
                .map(|env_name| !credentials.has_credential(env_name))
                .unwrap_or(false);
            ChainEntryView {
                provider: provider.clone(),
                credential: credential.map(|s| s.to_string()),
                skipped,
            }
        })
        .collect();
    ChainView {
        tool: tool.to_string(),
        entries,
        legal_providers: legal.iter().map(|s| s.to_string()).collect(),
    }
}

/// Pure(-ish) core of [`get_tools_settings`] — no disk I/O beyond what its
/// caller already resolved; directly unit-testable with a hand-built
/// `Config` and a `ToolCredentials` snapshot.
#[cfg(not(target_arch = "wasm32"))]
fn build_tools_settings_view(
    config: &ironhermes_core::config::Config,
    gate_open: bool,
    credentials: &ironhermes_tools::credentials::ToolCredentials,
) -> ToolsSettingsView {
    let timeout_overrides = config
        .tools
        .timeout_overrides
        .iter()
        .map(|(tool, seconds)| TimeoutOverrideRow {
            tool: tool.clone(),
            seconds: *seconds,
        })
        .collect();

    let chains = vec![
        build_chain_view(
            "web_search",
            &config.tools.web_search.chain,
            &ironhermes_core::config::WEB_SEARCH_PROVIDERS,
            credentials,
        ),
        build_chain_view(
            "web_answer",
            &config.tools.web_answer.chain,
            &ironhermes_core::config::WEB_ANSWER_PROVIDERS,
            credentials,
        ),
        build_chain_view(
            "web_extract",
            &config.tools.web_extract.chain,
            &ironhermes_core::config::WEB_EXTRACT_PROVIDERS,
            credentials,
        ),
    ];

    ToolsSettingsView {
        gate_open,
        timeout_secs: config.tools.timeout_secs,
        timeout_overrides,
        chains,
    }
}

/// Pure(-ish) core of [`update_tools_settings`] — `known_tool_names`
/// injected rather than read from the live registry (test-reachability
/// discipline, module doc). Staged-write order: validate -> fresh
/// scope-resolved disk read -> ROOT write gate -> mutate -> atomic save ->
/// live apply.
#[cfg(not(target_arch = "wasm32"))]
async fn update_tools_settings_with_known_tools(
    scope: ConfigScope,
    payload: ToolsSettingsPayload,
    known_tool_names: &std::collections::HashSet<String>,
) -> Result<(), Vec<String>> {
    // Step 1 — validate BEFORE any disk I/O.
    validate_tools_settings(&payload, known_tool_names)?;

    // Step 2 — fresh, scope-resolved disk read (never the startup snapshot).
    let (mut config, target) = resolve_scope_target(&scope).map_err(|e| vec![e])?;

    // Step 3 — fail-closed write gate, read from ROOT regardless of the
    // scope being edited (REVIEWS finding 7 / T-48.2-03-01 — a profile
    // without a config.yaml defaults this key to false, which would make
    // profile editing permanently impossible if the gate were re-derived
    // from the scope-resolved config).
    check_tools_write_gate().map_err(|e| vec![e])?;

    // Step 4 — mutate ONLY timeout_secs/timeout_overrides/the three chains,
    // then atomic save.
    merge_tools_settings(&mut config, &payload);
    save_scoped(&config, &target).map_err(|e| vec![e])?;

    // Step 5 — live apply (D-12 precedent): a root-scope save is visible to
    // the running agent without a restart.
    apply_live_toolset_config(&scope, &config).await;

    Ok(())
}

/// Read the staged settings view for `scope` (timeouts, overrides, and the
/// three provider chains with their skipped-for-missing-credential answer).
#[server]
pub async fn get_tools_settings(scope: ConfigScope) -> Result<ToolsSettingsView, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (config, _target) = resolve_scope_target(&scope).map_err(ServerFnError::new)?;
        let gate_open = root_gate_open();
        let credentials = resolve_tool_credentials(&config)
            .await
            .map_err(ServerFnError::new)?;
        Ok(build_tools_settings_view(&config, gate_open, &credentials))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scope;
        unreachable!("server fn body never runs on the wasm client")
    }
}

/// Staged-write commit for timeouts and all three provider chains (D-05/
/// D-11/D-21) — one validated, gated, atomic save that touches nothing else
/// in `tools:`.
#[server]
pub async fn update_tools_settings(
    scope: ConfigScope,
    payload: ToolsSettingsPayload,
) -> Result<(), ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let known_tool_names: std::collections::HashSet<String> = live_catalog_rows()
            .await
            .into_iter()
            .map(|r| r.name)
            .collect();
        update_tools_settings_with_known_tools(scope, payload, &known_tool_names)
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
    use ironhermes_core::config::{Config, SecurityConfig, ToolsConfig, ToolsetEntry};
    use ironhermes_tools::registry::{Prerequisite, ToolCatalogRow};
    use std::collections::HashSet;

    fn row(name: &str, group: &str, toolset: &str, available: bool) -> ToolCatalogRow {
        ToolCatalogRow {
            name: name.to_string(),
            description: format!("{name} description"),
            toolset: toolset.to_string(),
            group: group.to_string(),
            available,
            missing_prerequisites: if available {
                vec![]
            } else {
                vec![Prerequisite::env_var(
                    "SOME_MISSING_ENV_VAR",
                    "test-only missing prerequisite",
                    true,
                )]
            },
            // Phase 48.2 Plan 11: every existing caller of this helper
            // predates `runtime_dependency` and asserts nothing about it —
            // `None` keeps their behavior unchanged. The DTO passthrough
            // gets its own dedicated row below.
            runtime_dependency: None,
        }
    }

    // -------------------------------------------------------------------
    // build_tools_page_state — pure mapping tests
    // -------------------------------------------------------------------

    /// The write gate defaults closed — mirrors the provider-secrets gate
    /// test precedent (`provider_secrets_api.rs`).
    #[test]
    fn gate_closed_by_default() {
        let cfg = Config::default();
        assert!(
            !cfg.security.web_config_write_enabled,
            "SecurityConfig::default() must default to closed"
        );
        let state = build_tools_page_state(vec![], &cfg, false, ConfigScope::Root);
        assert!(!state.gate_open);
    }

    /// A disabled toolset's tools are still present in the mapped groups
    /// (D-16 — the unfiltered read).
    #[test]
    fn disabled_toolset_tools_still_present_in_mapped_groups() {
        let mut cfg = Config::default();
        // "code" is disabled by default; do not enable it.
        cfg.tools.toolsets.insert(
            "code".to_string(),
            ToolsetEntry { enabled: false },
        );
        let rows = vec![row("exec_python", "code", "code", true)];
        let state = build_tools_page_state(rows, &cfg, true, ConfigScope::Root);
        let group = state
            .toolsets
            .iter()
            .find(|g| g.name == "code")
            .expect("disabled toolset group must still be present");
        assert!(!group.enabled, "group must report the toolset's real disabled state");
        assert!(
            group.tools.iter().any(|t| t.name == "exec_python"),
            "tool must still be present inside the disabled group"
        );
    }

    /// A tool listed in `tools.disabled` is still present, with
    /// `disabled_override: true`.
    #[test]
    fn per_tool_disabled_tool_present_with_disabled_override_true() {
        let mut cfg = Config::default();
        cfg.tools.disabled.push("web_search".to_string());
        let rows = vec![row("web_search", "web", "web", true)];
        let state = build_tools_page_state(rows, &cfg, true, ConfigScope::Root);
        let entry = state
            .toolsets
            .iter()
            .flat_map(|g| g.tools.iter())
            .find(|t| t.name == "web_search")
            .expect("disabled tool must still be present");
        assert!(entry.disabled_override);
    }

    /// A missing required env-var prerequisite maps to `Unavailable` with the
    /// env var named.
    #[test]
    fn missing_required_prereq_maps_to_unavailable_with_env_var_named() {
        let cfg = Config::default();
        let rows = vec![row("some_tool", "web", "web", false)];
        let state = build_tools_page_state(rows, &cfg, true, ConfigScope::Root);
        let entry = state
            .toolsets
            .iter()
            .flat_map(|g| g.tools.iter())
            .find(|t| t.name == "some_tool")
            .expect("row must be present");
        match &entry.availability {
            ToolAvailability::Unavailable { missing } => {
                assert!(
                    missing.iter().any(|m| m.name == "SOME_MISSING_ENV_VAR"),
                    "missing prerequisites must name the env var; got: {missing:?}"
                );
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    /// The serialized `ToolsPageState` carries no credential value — only
    /// prerequisite kind/name/description (T-48.2-01-04).
    #[test]
    fn page_state_never_carries_a_credential_value() {
        let cfg = Config::default();
        let rows = vec![row("some_tool", "web", "web", false)];
        let state = build_tools_page_state(rows, &cfg, true, ConfigScope::Root);
        let json = serde_json::to_string(&state).expect("state must serialize");
        assert!(
            !json.to_lowercase().contains("api_key")
                && !json.to_lowercase().contains("secret"),
            "serialized page state must never carry a credential-shaped field; got: {json}"
        );
    }

    /// Every entry's `origin` equals its group's `kind` — a hand-built row
    /// set spanning one built-in group and one `mcp__<server>` group whose
    /// server key IS present in `config.mcp_servers` (Phase 48.2 Plan 10
    /// Task 2/D-20/G-48.2-4).
    #[test]
    fn every_entry_origin_equals_its_group_kind() {
        let mut cfg = Config::default();
        cfg.mcp_servers
            .insert("my-server".to_string(), serde_yaml::Value::Null);
        let rows = vec![
            row("exec_python", "code", "code", true),
            row("mcp_tool", "mcp__my_server", "mcp", true),
        ];
        let state = build_tools_page_state(rows, &cfg, true, ConfigScope::Root);

        for group in &state.toolsets {
            for tool in &group.tools {
                assert_eq!(
                    &tool.origin, &group.kind,
                    "entry {} origin must equal its group {}'s kind; got {:?} vs {:?}",
                    tool.name, group.name, tool.origin, group.kind
                );
            }
        }

        let builtin_group = state
            .toolsets
            .iter()
            .find(|g| g.name == "code")
            .expect("built-in group must be present");
        assert_eq!(builtin_group.kind, ToolsetKind::BuiltIn);
        assert_eq!(
            builtin_group.tools[0].origin,
            ToolsetKind::BuiltIn,
            "built-in card's origin must be BuiltIn"
        );

        let mcp_group = state
            .toolsets
            .iter()
            .find(|g| g.name == "mcp__my_server")
            .expect("mcp group must be present");
        assert_eq!(
            mcp_group.kind,
            ToolsetKind::Mcp {
                server: "my-server".to_string()
            }
        );
        assert_eq!(
            mcp_group.tools[0].origin,
            ToolsetKind::Mcp {
                server: "my-server".to_string()
            },
            "mcp card's origin must name the resolved server key, not the sanitized group suffix"
        );
    }

    /// An MCP group whose server key is absent from `config.mcp_servers`
    /// still stamps `Mcp { server }` on its cards, using the suffix-derived
    /// name — matching `resolve_group_kind_and_enabled`'s existing fallback
    /// (line ~381) rather than diverging from it.
    #[test]
    fn mcp_group_with_unmatched_server_key_stamps_suffix_derived_origin() {
        let cfg = Config::default(); // no mcp_servers entries at all
        let rows = vec![row("mcp_tool", "mcp__ghost_server", "mcp", true)];
        let state = build_tools_page_state(rows, &cfg, true, ConfigScope::Root);

        let mcp_group = state
            .toolsets
            .iter()
            .find(|g| g.name == "mcp__ghost_server")
            .expect("mcp group must be present even with an unmatched server key");
        assert_eq!(
            mcp_group.kind,
            ToolsetKind::Mcp {
                server: "ghost_server".to_string()
            },
            "unmatched server key must fall back to the raw suffix"
        );
        assert!(
            !mcp_group.enabled,
            "unmatched server key must report enabled: false"
        );
        assert_eq!(
            mcp_group.tools[0].origin,
            ToolsetKind::Mcp {
                server: "ghost_server".to_string()
            },
            "card origin must match the group's suffix-derived fallback kind"
        );
    }

    // -------------------------------------------------------------------
    // resolve_scope_target
    // -------------------------------------------------------------------

    /// `resolve_scope_target` for a `Profile` scope returns the
    /// profile-directory path, never the root path.
    #[test]
    fn resolve_scope_target_profile_returns_profile_directory_path() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let result = resolve_scope_target(&ConfigScope::Profile("my-profile".to_string()));

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        let (_config, target) = result.expect("profile scope with no config.yaml yet must load defaults, not error");
        let target_path = target.expect("profile scope must return Some(path)");
        assert!(
            target_path.ends_with("profiles/my-profile/config.yaml"),
            "target path must end with profiles/<name>/config.yaml; got: {}",
            target_path.display()
        );
    }

    // -------------------------------------------------------------------
    // mcp_group_server_key
    // -------------------------------------------------------------------

    /// `mcp_group_server_key` maps `mcp__my_server` back to the config key
    /// `my-server` (sanitized form differs from the raw key).
    #[test]
    fn mcp_group_server_key_recovers_raw_key_from_sanitized_group() {
        let mut mcp_servers = std::collections::HashMap::new();
        mcp_servers.insert("my-server".to_string(), serde_yaml::Value::Null);
        let recovered = mcp_group_server_key("mcp__my_server", &mcp_servers);
        assert_eq!(recovered, Some("my-server".to_string()));
    }

    #[test]
    fn mcp_group_server_key_returns_none_for_unmatched_group() {
        let mut mcp_servers = std::collections::HashMap::new();
        mcp_servers.insert("other-server".to_string(), serde_yaml::Value::Null);
        let recovered = mcp_group_server_key("mcp__my_server", &mcp_servers);
        assert_eq!(recovered, None);
    }

    // -------------------------------------------------------------------
    // toggle_toolset — validate / gate / apply
    // -------------------------------------------------------------------

    /// `toggle_toolset` rejects an `mcp__`-prefixed group before any disk
    /// I/O — no on-disk config bytes change.
    #[test]
    fn toggle_toolset_rejects_mcp_prefixed_group_before_any_disk_io() {
        let mut known_groups = HashSet::new();
        known_groups.insert("mcp__github".to_string());
        let result = validate_toolset_target("mcp__github", &known_groups);
        assert!(
            result.is_err(),
            "mcp__-prefixed group must be rejected even when it IS a known group"
        );
        assert!(
            result.unwrap_err().contains("MCP SERVERS"),
            "error must point to the MCP SERVERS section"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn toggle_toolset_end_to_end_rejects_mcp_prefixed_group_and_leaves_disk_unchanged() {
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
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let mut known_groups = HashSet::new();
        known_groups.insert("mcp__github".to_string());
        let result = toggle_toolset_with_known_groups(
            ConfigScope::Root,
            "mcp__github".to_string(),
            false,
            &known_groups,
        )
        .await;

        let after = std::fs::read(&config_path).expect("read config after rejected toggle");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err());
        assert_eq!(before, after, "on-disk config bytes must be unchanged");
    }

    #[test]
    fn toggle_toolset_rejects_unknown_toolset_name() {
        let known_groups: HashSet<String> = ["web".to_string()].into_iter().collect();
        let result = validate_toolset_target("not_a_real_toolset", &known_groups);
        assert!(result.is_err());
    }

    /// Gate-closed: `toggle_toolset` returns an error and the on-disk config
    /// bytes are unchanged.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn toggle_toolset_gate_closed_leaves_disk_unchanged() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        // Default config: web_config_write_enabled is false.
        let cfg = Config::default();
        cfg.save().expect("seed root config.yaml");
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let known_groups: HashSet<String> = ["web".to_string()].into_iter().collect();
        let result = toggle_toolset_with_known_groups(
            ConfigScope::Root,
            "web".to_string(),
            true,
            &known_groups,
        )
        .await;

        let after = std::fs::read(&config_path).expect("read config after gated toggle");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "gate-closed toggle must error");
        assert_eq!(before, after, "on-disk config bytes must be unchanged when the gate is closed");
    }

    /// The gate is answered from the ROOT config even when the scope is a
    /// profile: with the root gate open and a profile directory containing
    /// no `config.yaml`, a profile-scoped toolset toggle succeeds and
    /// creates that file.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn profile_scoped_toggle_succeeds_when_root_gate_open_and_profile_config_absent() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let root_cfg = Config {
            security: SecurityConfig {
                web_config_write_enabled: true,
                web_process_control_enabled: false,
                remote_blueprint_run_enabled: false,
            },
            ..Config::default()
        };
        root_cfg.save().expect("seed root config.yaml");

        let profile_config_path = home_dir
            .path()
            .join("profiles")
            .join("has-no-config-yet")
            .join("config.yaml");
        assert!(
            !profile_config_path.exists(),
            "precondition: profile config.yaml must not exist yet"
        );

        let known_groups: HashSet<String> = ["web".to_string()].into_iter().collect();
        let result = toggle_toolset_with_known_groups(
            ConfigScope::Profile("has-no-config-yet".to_string()),
            "web".to_string(),
            true,
            &known_groups,
        )
        .await;

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        result.expect("profile-scoped toggle must succeed when the ROOT gate is open");
        assert!(
            profile_config_path.exists(),
            "profile scoped save must create profiles/<name>/config.yaml"
        );
        let saved = ironhermes_core::config::Config::load_from(&profile_config_path)
            .expect("saved profile config must parse");
        assert!(saved.tools.is_toolset_enabled("web"));
    }

    /// `save_to` is used for the profile arm — never the root path.
    #[test]
    fn save_scoped_uses_save_to_for_profile_target() {
        let home_dir = tempfile::tempdir().expect("tempdir");
        let profile_path = home_dir.path().join("profiles/some-bot/config.yaml");
        let cfg = Config::default();
        save_scoped(&cfg, &Some(profile_path.clone())).expect("save_to profile path");
        assert!(profile_path.exists());
    }

    // -------------------------------------------------------------------
    // Task 2 (TDD): toggle_tool_disabled — <behavior> block, RED first.
    // -------------------------------------------------------------------

    /// Behavior 1: given a config whose `tools.disabled` contains
    /// `web_search`, `get_tools_page_state` (via `build_tools_page_state`)
    /// returns that entry with `disabled_override: true` and it is still
    /// inside its toolset group. Already covered by
    /// `per_tool_disabled_tool_present_with_disabled_override_true` above —
    /// this test re-asserts it under Task 2's own name so the `<behavior>`
    /// block has one test per case, not four-of-five.
    #[test]
    fn behavior_1_disabled_tool_present_with_disabled_override_true_and_inside_its_group() {
        let mut cfg = Config::default();
        cfg.tools.disabled.push("web_search".to_string());
        let rows = vec![row("web_search", "web", "web", true)];
        let state = build_tools_page_state(rows, &cfg, true, ConfigScope::Root);
        let group = state
            .toolsets
            .iter()
            .find(|g| g.name == "web")
            .expect("tool must still be inside its toolset group");
        let entry = group
            .tools
            .iter()
            .find(|t| t.name == "web_search")
            .expect("disabled tool must still be present");
        assert!(entry.disabled_override);
    }

    /// Behavior 2: `toggle_tool_disabled(Root, "web_search", true)` with the
    /// gate open adds exactly one `web_search` entry to `tools.disabled`,
    /// and a second identical call leaves the list unchanged (idempotent
    /// set, no duplicate).
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn behavior_2_disable_toggle_is_idempotent_no_duplicate() {
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

        let known_tools: HashSet<String> = ["web_search".to_string()].into_iter().collect();

        toggle_tool_disabled_with_known_tools(
            ConfigScope::Root,
            "web_search".to_string(),
            true,
            &known_tools,
        )
        .await
        .expect("first disable call must succeed");
        toggle_tool_disabled_with_known_tools(
            ConfigScope::Root,
            "web_search".to_string(),
            true,
            &known_tools,
        )
        .await
        .expect("second identical disable call must succeed (idempotent)");

        let after = ironhermes_core::config::Config::load().expect("reload config");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        let hits = after
            .tools
            .disabled
            .iter()
            .filter(|d| *d == "web_search")
            .count();
        assert_eq!(hits, 1, "must have exactly one web_search entry, not a duplicate");
    }

    /// Behavior 3: `toggle_tool_disabled(Root, "web_search", false)` removes
    /// `web_search` from `tools.disabled` and disturbs no other entry.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn behavior_3_enable_toggle_removes_only_the_named_tool() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let cfg = Config {
            security: SecurityConfig {
                web_config_write_enabled: true,
                web_process_control_enabled: false,
                remote_blueprint_run_enabled: false,
            },
            tools: ToolsConfig {
                disabled: vec!["web_search".to_string(), "browser_navigate".to_string()],
                ..ToolsConfig::default()
            },
            ..Config::default()
        };
        cfg.save().expect("seed root config.yaml");

        let known_tools: HashSet<String> = ["web_search".to_string()].into_iter().collect();
        toggle_tool_disabled_with_known_tools(
            ConfigScope::Root,
            "web_search".to_string(),
            false,
            &known_tools,
        )
        .await
        .expect("enable call must succeed");

        let after = ironhermes_core::config::Config::load().expect("reload config");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(!after.tools.disabled.iter().any(|d| d == "web_search"));
        assert!(
            after.tools.disabled.iter().any(|d| d == "browser_navigate"),
            "an unrelated disabled entry must not be disturbed"
        );
    }

    /// Behavior 4: given the gate closed, `toggle_tool_disabled` returns an
    /// error and `tools.disabled` on disk is byte-identical afterwards.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn behavior_4_gate_closed_leaves_disk_byte_identical() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        // Default config: web_config_write_enabled is false.
        let cfg = Config::default();
        cfg.save().expect("seed root config.yaml");
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let known_tools: HashSet<String> = ["web_search".to_string()].into_iter().collect();
        let result = toggle_tool_disabled_with_known_tools(
            ConfigScope::Root,
            "web_search".to_string(),
            true,
            &known_tools,
        )
        .await;

        let after = std::fs::read(&config_path).expect("read config after gated call");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "gate-closed call must error");
        assert_eq!(before, after, "on-disk bytes must be byte-identical when the gate is closed");
    }

    /// Behavior 5: given an unknown tool name, `toggle_tool_disabled` errors
    /// before any disk read or write. Pure — no I/O, no env var needed.
    #[test]
    fn behavior_5_unknown_tool_name_errors_before_any_disk_io() {
        let known_tools: HashSet<String> = ["web_search".to_string()].into_iter().collect();
        let result = validate_known_tool("not_a_real_tool", &known_tools);
        assert!(result.is_err());
    }

    // -------------------------------------------------------------------
    // Task 3 (48.2-03, TDD): validate_tools_settings / update_tools_settings
    // / get_tools_settings — <behavior> block, RED first.
    // -------------------------------------------------------------------

    fn default_valid_payload() -> ToolsSettingsPayload {
        use ironhermes_core::config::{WEB_ANSWER_PROVIDERS, WEB_EXTRACT_PROVIDERS, WEB_SEARCH_PROVIDERS};
        ToolsSettingsPayload {
            timeout_secs: 60,
            timeout_overrides: vec![],
            web_search: WEB_SEARCH_PROVIDERS.iter().map(|s| s.to_string()).collect(),
            web_answer: WEB_ANSWER_PROVIDERS.iter().map(|s| s.to_string()).collect(),
            web_extract: WEB_EXTRACT_PROVIDERS.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Behavior: `timeout_secs == 0` is rejected with a message naming the
    /// field; any positive value is accepted.
    #[test]
    fn behavior_timeout_secs_zero_rejected_positive_accepted() {
        let known_tools: HashSet<String> = HashSet::new();

        let mut zero_payload = default_valid_payload();
        zero_payload.timeout_secs = 0;
        let errors = validate_tools_settings(&zero_payload, &known_tools)
            .expect_err("timeout_secs == 0 must be rejected");
        assert!(
            errors.iter().any(|e| e.contains("timeout_secs")),
            "error must name timeout_secs; got: {errors:?}"
        );

        let positive_payload = default_valid_payload();
        assert!(
            validate_tools_settings(&positive_payload, &known_tools).is_ok(),
            "a positive timeout_secs with legal chains must be accepted"
        );
    }

    /// Behavior: a negative `timeout_overrides` value is accepted (the
    /// documented disable-the-bound convention); an override key that is
    /// not a registered tool name is rejected.
    #[test]
    fn behavior_negative_override_accepted_unregistered_key_rejected() {
        let known_tools: HashSet<String> = ["web_search".to_string()].into_iter().collect();

        let mut negative_payload = default_valid_payload();
        negative_payload.timeout_overrides = vec![TimeoutOverrideRow {
            tool: "web_search".to_string(),
            seconds: -1,
        }];
        assert!(
            validate_tools_settings(&negative_payload, &known_tools).is_ok(),
            "a negative per-tool override must be accepted (disable-the-bound convention)"
        );

        let mut unknown_key_payload = default_valid_payload();
        unknown_key_payload.timeout_overrides = vec![TimeoutOverrideRow {
            tool: "not_a_real_tool".to_string(),
            seconds: 30,
        }];
        let errors = validate_tools_settings(&unknown_key_payload, &known_tools)
            .expect_err("an override key that is not a registered tool name must be rejected");
        assert!(
            errors.iter().any(|e| e.contains("not_a_real_tool")),
            "error must name the offending key; got: {errors:?}"
        );
    }

    /// Behavior: a payload with an unknown provider in `web_search` AND an
    /// empty `web_extract` chain returns exactly two messages, one naming
    /// each problem.
    #[test]
    fn behavior_two_chain_problems_produce_exactly_two_messages_naming_each() {
        let known_tools: HashSet<String> = HashSet::new();
        let mut payload = default_valid_payload();
        payload.web_search = vec!["not_a_real_provider".to_string()];
        payload.web_extract = vec![];

        let errors = validate_tools_settings(&payload, &known_tools)
            .expect_err("two independent chain problems must fail validation");
        assert_eq!(
            errors.len(),
            2,
            "must produce exactly two messages, got: {errors:?}"
        );
        assert!(
            errors.iter().any(|e| e.contains("web_search")),
            "one message must name web_search; got: {errors:?}"
        );
        assert!(
            errors.iter().any(|e| e.contains("web_extract")),
            "one message must name web_extract; got: {errors:?}"
        );
    }

    /// Behavior: `update_tools_settings` with the write gate closed returns
    /// an error and leaves the on-disk config bytes unchanged.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn behavior_update_gate_closed_leaves_disk_unchanged() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        // Default config: web_config_write_enabled is false.
        let cfg = Config::default();
        cfg.save().expect("seed root config.yaml");
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let known_tools: HashSet<String> = HashSet::new();
        let payload = default_valid_payload();
        let result =
            update_tools_settings_with_known_tools(ConfigScope::Root, payload, &known_tools)
                .await;

        let after = std::fs::read(&config_path).expect("read config after gate-closed call");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "gate-closed update must error");
        assert_eq!(
            before, after,
            "on-disk bytes must be unchanged when the gate is closed"
        );
    }

    /// Behavior: `update_tools_settings` with a validation failure leaves
    /// the on-disk config bytes unchanged.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn behavior_update_validation_failure_leaves_disk_unchanged() {
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
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let known_tools: HashSet<String> = HashSet::new();
        let mut payload = default_valid_payload();
        payload.timeout_secs = 0; // invalid

        let result =
            update_tools_settings_with_known_tools(ConfigScope::Root, payload, &known_tools)
                .await;

        let after = std::fs::read(&config_path).expect("read config after invalid payload");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "invalid payload must error");
        assert_eq!(
            before, after,
            "on-disk bytes must be unchanged when validation fails"
        );
    }

    /// Behavior: `update_tools_settings` with a valid payload writes
    /// `timeout_secs`, `timeout_overrides` and all three chains, and leaves
    /// `tools.toolsets`, `tools.disabled` and `tools.credentials` untouched.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn behavior_update_valid_payload_writes_settings_leaves_other_fields_untouched() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let mut seed_toolsets = std::collections::HashMap::new();
        seed_toolsets.insert("web".to_string(), ToolsetEntry { enabled: true });
        let mut seed_credentials = std::collections::BTreeMap::new();
        seed_credentials.insert("EXA_API_KEY".to_string(), "seeded-value".to_string());

        let cfg = Config {
            security: SecurityConfig {
                web_config_write_enabled: true,
                web_process_control_enabled: false,
                remote_blueprint_run_enabled: false,
            },
            tools: ToolsConfig {
                toolsets: seed_toolsets.clone(),
                disabled: vec!["browser_navigate".to_string()],
                credentials: seed_credentials.clone(),
                ..ToolsConfig::default()
            },
            ..Config::default()
        };
        cfg.save().expect("seed root config.yaml");

        let known_tools: HashSet<String> = ["web_search".to_string()].into_iter().collect();
        let mut payload = default_valid_payload();
        payload.timeout_secs = 45;
        payload.timeout_overrides = vec![TimeoutOverrideRow {
            tool: "web_search".to_string(),
            seconds: -1,
        }];
        payload.web_search = vec!["brave".to_string(), "ddg".to_string()];

        update_tools_settings_with_known_tools(ConfigScope::Root, payload, &known_tools)
            .await
            .expect("valid payload must succeed");

        let after = ironhermes_core::config::Config::load().expect("reload config");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert_eq!(after.tools.timeout_secs, 45);
        assert_eq!(
            after.tools.timeout_overrides.get("web_search"),
            Some(&-1)
        );
        assert_eq!(
            after.tools.web_search.chain,
            vec!["brave".to_string(), "ddg".to_string()]
        );
        // ToolsetEntry has no PartialEq — compare field-by-field instead of
        // the whole HashMap.
        assert_eq!(
            after.tools.toolsets.len(),
            seed_toolsets.len(),
            "tools.toolsets must not gain or lose entries from a settings save"
        );
        assert_eq!(
            after.tools.toolsets.get("web").map(|e| e.enabled),
            seed_toolsets.get("web").map(|e| e.enabled),
            "tools.toolsets must be untouched by a settings save"
        );
        assert_eq!(
            after.tools.disabled,
            vec!["browser_navigate".to_string()],
            "tools.disabled must be untouched by a settings save"
        );
        assert_eq!(
            after.tools.credentials, seed_credentials,
            "tools.credentials must be untouched by a settings save"
        );
    }

    /// T-48.2-03-01 / REVIEWS finding 7: the gate is answered from the ROOT
    /// config even when the scope is a profile — with the root gate open
    /// and a profile directory containing no `config.yaml`, a
    /// profile-scoped `update_tools_settings` succeeds and creates that
    /// file.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn profile_scoped_settings_save_succeeds_when_root_gate_open_and_profile_config_absent()
    {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let root_cfg = Config {
            security: SecurityConfig {
                web_config_write_enabled: true,
                web_process_control_enabled: false,
                remote_blueprint_run_enabled: false,
            },
            ..Config::default()
        };
        root_cfg.save().expect("seed root config.yaml");

        let profile_config_path = home_dir
            .path()
            .join("profiles")
            .join("settings-no-config-yet")
            .join("config.yaml");
        assert!(
            !profile_config_path.exists(),
            "precondition: profile config.yaml must not exist yet"
        );

        let known_tools: HashSet<String> = HashSet::new();
        let payload = default_valid_payload();
        let result = update_tools_settings_with_known_tools(
            ConfigScope::Profile("settings-no-config-yet".to_string()),
            payload,
            &known_tools,
        )
        .await;

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        result.expect("profile-scoped settings save must succeed when the ROOT gate is open");
        assert!(
            profile_config_path.exists(),
            "profile scoped save must create profiles/<name>/config.yaml"
        );
    }

    /// Behavior: `get_tools_settings` (via `build_tools_settings_view`)
    /// marks a chain entry whose canonical credential is absent as skipped
    /// and names that credential; an entry whose provider needs no
    /// credential is never marked skipped.
    #[test]
    fn behavior_chain_entry_missing_credential_is_skipped_and_named() {
        let _g = crate::server::test_support::env_lock();
        let prior_exa = std::env::var("EXA_API_KEY").ok();
        unsafe { std::env::remove_var("EXA_API_KEY") };

        let mut cfg = Config::default();
        cfg.tools.web_search.chain = vec!["exa".to_string(), "ddg".to_string()];
        let credentials = ironhermes_tools::credentials::ToolCredentials::env_only();
        let view = build_tools_settings_view(&cfg, true, &credentials);

        if let Some(v) = prior_exa {
            unsafe { std::env::set_var("EXA_API_KEY", v) };
        }

        let chain = view
            .chains
            .iter()
            .find(|c| c.tool == "web_search")
            .expect("web_search chain must be present");

        let exa_entry = chain
            .entries
            .iter()
            .find(|e| e.provider == "exa")
            .expect("exa entry must be present");
        assert!(
            exa_entry.skipped,
            "exa entry must be skipped when EXA_API_KEY is absent"
        );
        assert_eq!(exa_entry.credential.as_deref(), Some("EXA_API_KEY"));

        let ddg_entry = chain
            .entries
            .iter()
            .find(|e| e.provider == "ddg")
            .expect("ddg entry must be present");
        assert!(
            !ddg_entry.skipped,
            "an entry whose provider needs no credential is never marked skipped"
        );
        assert_eq!(ddg_entry.credential, None);
    }

    // -------------------------------------------------------------------
    // Phase 48.2 Plan 05 (D-08 isolation): a profile-scoped write never
    // touches root, and a root-scoped write never touches a profile —
    // proven bidirectionally by comparing raw on-disk BYTES, not just
    // parsed config equality. This is the load-bearing check for the
    // whole profile dimension.
    // -------------------------------------------------------------------

    /// T-48.2-05-02: a profile-scoped toolset toggle lands in the
    /// profile's own config.yaml and leaves the root config.yaml BYTES
    /// unchanged.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn profile_scoped_toolset_toggle_writes_profile_and_leaves_root_byte_identical() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let root_cfg = Config {
            security: SecurityConfig {
                web_config_write_enabled: true,
                web_process_control_enabled: false,
                remote_blueprint_run_enabled: false,
            },
            ..Config::default()
        };
        root_cfg.save().expect("seed root config.yaml");
        let root_path = ironhermes_core::config::Config::config_path();
        let root_before = std::fs::read(&root_path).expect("read seeded root config");

        // Seed the profile's own config.yaml so this exercises a mutation
        // of an EXISTING file, not the create-on-first-write path already
        // covered by `profile_scoped_toggle_succeeds_when_root_gate_open_and_profile_config_absent`.
        let profile_path = home_dir
            .path()
            .join("profiles")
            .join("isolation-profile")
            .join("config.yaml");
        Config::default()
            .save_to(&profile_path)
            .expect("seed profile config.yaml");

        let known_groups: HashSet<String> = ["web".to_string()].into_iter().collect();
        let result = toggle_toolset_with_known_groups(
            ConfigScope::Profile("isolation-profile".to_string()),
            "web".to_string(),
            true,
            &known_groups,
        )
        .await;

        let root_after = std::fs::read(&root_path).expect("read root config after profile toggle");

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        result.expect("profile-scoped toggle must succeed with the root gate open");
        let profile_after = ironhermes_core::config::Config::load_from(&profile_path)
            .expect("saved profile config must parse");
        assert!(
            profile_after.tools.is_toolset_enabled("web"),
            "the profile's own config.yaml must carry the new toolset state"
        );
        assert_eq!(
            root_before, root_after,
            "a profile-scoped write must never change the root config.yaml bytes"
        );
    }

    /// Mirror of the above: a root-scoped toolset toggle leaves an
    /// existing profile's config.yaml BYTES unchanged.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn root_scoped_toolset_toggle_leaves_profile_byte_identical() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let root_cfg = Config {
            security: SecurityConfig {
                web_config_write_enabled: true,
                web_process_control_enabled: false,
                remote_blueprint_run_enabled: false,
            },
            ..Config::default()
        };
        root_cfg.save().expect("seed root config.yaml");

        let profile_path = home_dir
            .path()
            .join("profiles")
            .join("isolation-profile-2")
            .join("config.yaml");
        Config::default()
            .save_to(&profile_path)
            .expect("seed profile config.yaml");
        let profile_before = std::fs::read(&profile_path).expect("read seeded profile config");

        let known_groups: HashSet<String> = ["web".to_string()].into_iter().collect();
        let result = toggle_toolset_with_known_groups(
            ConfigScope::Root,
            "web".to_string(),
            true,
            &known_groups,
        )
        .await;

        let profile_after =
            std::fs::read(&profile_path).expect("read profile config after root toggle");

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        result.expect("root-scoped toggle must succeed with the gate open");
        assert_eq!(
            profile_before, profile_after,
            "a root-scoped write must never change an existing profile's config.yaml bytes"
        );
    }

    /// `credential_for_chain_provider` returns a decision for every name in
    /// each of the three per-tool legal provider constants — a future
    /// provider added to the core crate cannot silently render without a
    /// credential answer.
    #[test]
    fn credential_for_chain_provider_handles_every_legal_provider() {
        use ironhermes_core::config::{
            WEB_ANSWER_PROVIDERS, WEB_EXTRACT_PROVIDERS, WEB_SEARCH_PROVIDERS,
        };
        for provider in WEB_SEARCH_PROVIDERS
            .iter()
            .chain(WEB_ANSWER_PROVIDERS.iter())
            .chain(WEB_EXTRACT_PROVIDERS.iter())
        {
            match *provider {
                "ddg" | "local" => assert_eq!(
                    credential_for_chain_provider(provider),
                    None,
                    "{provider} needs no credential"
                ),
                other => assert!(
                    credential_for_chain_provider(other).is_some(),
                    "{other} must map to a credential env var"
                ),
            }
        }
    }

    // -------------------------------------------------------------------
    // Phase 48.2 Plan 06 Task 2 (D-17, TDD): bulk_targets / set_toolsets_bulk
    // — <behavior> block, RED first.
    // -------------------------------------------------------------------

    fn builtin_group(name: &str, enabled: bool) -> ToolsetGroup {
        ToolsetGroup {
            name: name.to_string(),
            kind: ToolsetKind::BuiltIn,
            enabled,
            tools: vec![],
        }
    }

    fn mcp_group(name: &str, server: &str) -> ToolsetGroup {
        ToolsetGroup {
            name: name.to_string(),
            kind: ToolsetKind::Mcp { server: server.to_string() },
            enabled: true,
            tools: vec![],
        }
    }

    /// Behavior: `bulk_targets` over a catalog containing built-in
    /// toolsets, one `ToolsetKind::Mcp` group, and a group named with the
    /// flat `"mcp"` sentinel returns only the built-in names.
    #[test]
    fn bulk_targets_excludes_mcp_display_group_and_mcp_sentinel() {
        let groups = vec![
            builtin_group("web", false),
            builtin_group("code", false),
            mcp_group("mcp__github", "github"),
            builtin_group("mcp", true), // flat sentinel, wrongly tagged BuiltIn here to prove ALL_TOOLSETS gate alone rejects it
        ];
        let targets = bulk_targets(&groups);
        assert!(targets.contains(&"web".to_string()));
        assert!(targets.contains(&"code".to_string()));
        assert!(!targets.contains(&"mcp__github".to_string()));
        assert!(
            !targets.contains(&"mcp".to_string()),
            "the flat mcp sentinel must never be a bulk target even if mis-tagged BuiltIn"
        );
    }

    /// Behavior: `bulk_targets` excludes a group named with the `"kanban"`
    /// sentinel even though it is not MCP — it is not in `ALL_TOOLSETS`.
    #[test]
    fn bulk_targets_excludes_kanban_sentinel_even_though_builtin_kind() {
        let groups = vec![builtin_group("web", false), builtin_group("kanban", true)];
        let targets = bulk_targets(&groups);
        assert!(targets.contains(&"web".to_string()));
        assert!(!targets.contains(&"kanban".to_string()));
    }

    /// Behavior: `bulk_targets` returns only names present in
    /// `ALL_TOOLSETS`, sorted, with no duplicates.
    #[test]
    fn bulk_targets_returns_only_all_toolsets_members_sorted_deduped() {
        let groups = vec![
            builtin_group("web", false),
            builtin_group("web", false), // duplicate group, e.g. a bad catalog read
            builtin_group("agent", true),
            builtin_group("not_a_real_toolset", true),
        ];
        let targets = bulk_targets(&groups);
        assert!(
            targets
                .iter()
                .all(|t| ironhermes_core::constants::ALL_TOOLSETS.contains(&t.as_str())),
            "every returned target must be an ALL_TOOLSETS member; got {targets:?}"
        );
        let mut sorted = targets.clone();
        sorted.sort();
        assert_eq!(targets, sorted, "targets must be sorted");
        let unique: HashSet<&String> = targets.iter().collect();
        assert_eq!(unique.len(), targets.len(), "targets must be de-duplicated");
    }

    fn seed_config_with_gate_open() -> Config {
        Config {
            security: SecurityConfig {
                web_config_write_enabled: true,
                web_process_control_enabled: false,
                remote_blueprint_run_enabled: false,
            },
            ..Config::default()
        }
    }

    /// Behavior: `set_toolsets_bulk` with `enable: true` sets every
    /// returned target to enabled and leaves any `mcp__`-prefixed, `"mcp"`
    /// or `"kanban"` toolsets entry exactly as it was.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn bulk_enable_sets_every_target_and_leaves_mcp_and_kanban_entries_untouched() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let mut cfg = seed_config_with_gate_open();
        // Pre-seed sentinel + MCP entries that must survive byte-identical.
        cfg.tools.toolsets.insert("mcp__github".to_string(), ToolsetEntry { enabled: false });
        cfg.tools.toolsets.insert("mcp".to_string(), ToolsetEntry { enabled: true });
        cfg.tools.toolsets.insert("kanban".to_string(), ToolsetEntry { enabled: true });
        cfg.save().expect("seed root config.yaml");

        let groups = vec![
            builtin_group("web", false),
            builtin_group("code", false),
            builtin_group("browser", false),
            mcp_group("mcp__github", "github"),
        ];
        let outcomes = set_toolsets_bulk_with_groups(ConfigScope::Root, true, &groups)
            .await
            .expect("bulk enable must succeed with the gate open");

        let after = ironhermes_core::config::Config::load().expect("reload config");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        for target in ["web", "code", "browser"] {
            assert!(
                after.tools.is_toolset_enabled(target),
                "{target} must be enabled after bulk enable"
            );
        }
        assert!(
            outcomes.iter().all(|o| o.enabled),
            "every outcome must report enabled: true"
        );
        assert_eq!(
            after.tools.toolsets.get("mcp__github").map(|e| e.enabled),
            Some(false),
            "an mcp__-prefixed entry must be untouched by a bulk enable"
        );
        assert_eq!(
            after.tools.toolsets.get("mcp").map(|e| e.enabled),
            Some(true),
            "the mcp sentinel entry must be untouched by a bulk enable"
        );
        assert_eq!(
            after.tools.toolsets.get("kanban").map(|e| e.enabled),
            Some(true),
            "the kanban sentinel entry must be untouched by a bulk enable"
        );
    }

    /// Behavior: `set_toolsets_bulk` with `enable: false` sets every
    /// returned target to disabled and leaves those same entries exactly
    /// as they were.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn bulk_disable_sets_every_target_and_leaves_mcp_and_kanban_entries_untouched() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let mut cfg = seed_config_with_gate_open();
        cfg.tools.toolsets.insert("memory".to_string(), ToolsetEntry { enabled: true });
        cfg.tools.toolsets.insert("mcp__github".to_string(), ToolsetEntry { enabled: true });
        cfg.tools.toolsets.insert("mcp".to_string(), ToolsetEntry { enabled: true });
        cfg.tools.toolsets.insert("kanban".to_string(), ToolsetEntry { enabled: true });
        cfg.save().expect("seed root config.yaml");

        let groups = vec![builtin_group("memory", true), mcp_group("mcp__github", "github")];
        let outcomes = set_toolsets_bulk_with_groups(ConfigScope::Root, false, &groups)
            .await
            .expect("bulk disable must succeed with the gate open");

        let after = ironhermes_core::config::Config::load().expect("reload config");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(!after.tools.is_toolset_enabled("memory"));
        assert!(outcomes.iter().all(|o| !o.enabled));
        assert_eq!(after.tools.toolsets.get("mcp__github").map(|e| e.enabled), Some(true));
        assert_eq!(after.tools.toolsets.get("mcp").map(|e| e.enabled), Some(true));
        assert_eq!(after.tools.toolsets.get("kanban").map(|e| e.enabled), Some(true));
    }

    /// Behavior: `set_toolsets_bulk` returns one outcome entry per target
    /// naming the toolset and whether it changed.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn bulk_outcomes_report_one_entry_per_target_and_whether_it_changed() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let mut cfg = seed_config_with_gate_open();
        cfg.tools.toolsets.insert("web".to_string(), ToolsetEntry { enabled: true });
        cfg.tools.toolsets.insert("code".to_string(), ToolsetEntry { enabled: false });
        cfg.save().expect("seed root config.yaml");

        let groups = vec![builtin_group("web", true), builtin_group("code", false)];
        let outcomes = set_toolsets_bulk_with_groups(ConfigScope::Root, true, &groups)
            .await
            .expect("bulk enable must succeed");

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert_eq!(outcomes.len(), 2, "one outcome per target");
        let web_outcome = outcomes.iter().find(|o| o.toolset == "web").expect("web outcome present");
        assert!(!web_outcome.changed, "web was already enabled — not changed");
        let code_outcome = outcomes.iter().find(|o| o.toolset == "code").expect("code outcome present");
        assert!(code_outcome.changed, "code was disabled — must report changed");
    }

    /// Behavior: `set_toolsets_bulk` with the write gate closed returns an
    /// error and leaves the on-disk config bytes unchanged.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn bulk_gate_closed_leaves_disk_unchanged() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        // Default config: web_config_write_enabled is false.
        let cfg = Config::default();
        cfg.save().expect("seed root config.yaml");
        let config_path = ironhermes_core::config::Config::config_path();
        let before = std::fs::read(&config_path).expect("read seeded config");

        let groups = vec![builtin_group("web", false)];
        let result = set_toolsets_bulk_with_groups(ConfigScope::Root, true, &groups).await;

        let after = std::fs::read(&config_path).expect("read config after gated bulk call");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "gate-closed bulk call must error");
        assert_eq!(before, after, "on-disk config bytes must be unchanged when the gate is closed");
    }

    /// Behavior: `set_toolsets_bulk` leaves `tools.disabled`,
    /// `tools.credentials`, `tools.timeout_secs` and all three chains
    /// untouched.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn bulk_leaves_disabled_credentials_timeout_and_chains_untouched() {
        let _g = crate::server::test_support::env_lock();
        let home_dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", home_dir.path()) };

        let mut seed_credentials = std::collections::BTreeMap::new();
        seed_credentials.insert("EXA_API_KEY".to_string(), "seeded-value".to_string());

        let mut cfg = seed_config_with_gate_open();
        cfg.tools.disabled = vec!["browser_navigate".to_string()];
        cfg.tools.credentials = seed_credentials.clone();
        cfg.tools.timeout_secs = 77;
        cfg.tools.web_search.chain = vec!["brave".to_string(), "ddg".to_string()];
        cfg.save().expect("seed root config.yaml");

        let groups = vec![builtin_group("web", false)];
        set_toolsets_bulk_with_groups(ConfigScope::Root, true, &groups)
            .await
            .expect("bulk enable must succeed");

        let after = ironhermes_core::config::Config::load().expect("reload config");
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert_eq!(after.tools.disabled, vec!["browser_navigate".to_string()]);
        assert_eq!(after.tools.credentials, seed_credentials);
        assert_eq!(after.tools.timeout_secs, 77);
        assert_eq!(after.tools.web_search.chain, vec!["brave".to_string(), "ddg".to_string()]);
    }

    /// Every name in `HIGH_BLAST_RADIUS_TOOLSETS` is present in the default
    /// toolsets map and ships disabled there — so the constant cannot drift
    /// from the core crate's own defaults unnoticed.
    #[test]
    fn high_blast_radius_toolsets_match_core_defaults() {
        assert_eq!(HIGH_BLAST_RADIUS_TOOLSETS.len(), 3);
        let cfg = Config::default();
        for name in HIGH_BLAST_RADIUS_TOOLSETS {
            assert!(
                cfg.tools.toolsets.contains_key(*name),
                "{name} must be present in the default toolsets map"
            );
            assert!(
                !cfg.tools.is_toolset_enabled(name),
                "{name} must ship disabled by default (high-blast-radius)"
            );
        }
    }

    // -------------------------------------------------------------------
    // Phase 48.2 Plan 11 (G-48.2-6 slice a): runtime_dependency drift guard
    // + DTO passthrough
    // -------------------------------------------------------------------

    #[test]
    fn portable_gateway_runtime_dependency_matches_source_of_truth() {
        assert_eq!(
            GATEWAY_RUNTIME_DEPENDENCY,
            ironhermes_tools::registry::GATEWAY_RUNTIME_DEPENDENCY,
            "the portable wasm-safe copy must stay byte-identical to ironhermes_tools::registry::GATEWAY_RUNTIME_DEPENDENCY"
        );
    }

    /// `build_tools_page_state` projects `ToolCatalogRow::runtime_dependency`
    /// onto `ToolCatalogEntry::runtime_dependency` unchanged — `Some` stays
    /// `Some` (converted to an owned `String`), `None` stays `None`.
    #[test]
    fn build_tools_page_state_carries_runtime_dependency_through() {
        let mut with_dep = row("cronjob", "agent", "agent", true);
        with_dep.runtime_dependency = Some(ironhermes_tools::registry::GATEWAY_RUNTIME_DEPENDENCY);
        let without_dep = row("delegate_task", "agent", "agent", true);

        let config = Config::default();
        let state = build_tools_page_state(
            vec![with_dep, without_dep],
            &config,
            true,
            ConfigScope::Root,
        );

        let cards: Vec<&ToolCatalogEntry> =
            state.toolsets.iter().flat_map(|g| g.tools.iter()).collect();
        let cronjob_card = cards
            .iter()
            .find(|c| c.name == "cronjob")
            .expect("cronjob card");
        let delegate_card = cards
            .iter()
            .find(|c| c.name == "delegate_task")
            .expect("delegate_task card");

        assert_eq!(cronjob_card.runtime_dependency.as_deref(), Some("gateway"));
        assert_eq!(delegate_card.runtime_dependency, None);
    }

    /// D-12 static-source invariant (GAP-1, gsd-nyquist-auditor 48.2): every
    /// PRODUCTION `save_scoped(` call site in this file must be paired with a
    /// `apply_live_toolset_config(` call, so a future write path can never
    /// ship a silently-stale running agent. Follows the `include_str!` static
    /// scan pattern established in `mcp_admin_api.rs` /
    /// `ironhermes-mcp/src/manager.rs`. Scoped to production code only (cut
    /// at `mod tests`) and comment-stripped so doc prose mentioning the
    /// helper names cannot satisfy or defeat the count.
    #[test]
    fn every_save_scoped_call_is_paired_with_apply_live_toolset_config_d12() {
        let src = include_str!("tools_config_api.rs");

        // Cut at the `mod tests {` line (line-start-anchored so a string
        // literal or comment containing the same text mid-line can't be
        // mistaken for the module boundary).
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
        // contract (which legitimately mention both helper names) cannot
        // satisfy or defeat this assertion.
        let code_only: String = production_src
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        let save_scoped_calls = code_only.matches("save_scoped(").count();
        let apply_live_calls = code_only.matches("apply_live_toolset_config(").count();

        // Exclude the two definition sites — `fn save_scoped(` and
        // `async fn apply_live_toolset_config(` — from the call counts.
        let save_scoped_def = code_only.matches("fn save_scoped(").count();
        let apply_live_def = code_only
            .matches("async fn apply_live_toolset_config(")
            .count();

        let save_scoped_call_count = save_scoped_calls - save_scoped_def;
        let apply_live_call_count = apply_live_calls - apply_live_def;

        assert_eq!(
            save_scoped_call_count, apply_live_call_count,
            "D-12 violation: this file has {save_scoped_call_count} production `save_scoped(` \
             call site(s) but {apply_live_call_count} `apply_live_toolset_config(` call site(s) \
             — a write path was added without a matching live-apply call, which would ship a \
             silently-stale running agent"
        );
    }
}

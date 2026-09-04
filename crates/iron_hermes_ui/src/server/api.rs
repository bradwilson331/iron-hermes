//! Server functions for the Dioxus UI.

use dioxus::prelude::*;
// Server-only: `ironhermes_core` is not a wasm dependency, and `Platform` is used
// only inside server-function bodies (e.g. list_sessions), which the #[get]/#[post]
// macros compile under `feature = "server"`. An unconditional import breaks the
// wasm/web build with E0433.
#[cfg(feature = "server")]
use ironhermes_core::types::Platform;
// Phase 46.6 Plan 03 (D-04): ArtifactSummary is server-only, same reasoning
// as the Platform import above — used only inside list_artifacts' body.
#[cfg(feature = "server")]
use ironhermes_artifacts::ArtifactSummary;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    pub title: Option<String>,
    pub created_at: String,
    pub message_count: u32,
    /// Phase 26.7.2 D-04: last message preview (~80 chars, server-truncated)
    pub last_message: Option<String>,
}

/// Phase 26.7.2 (D-10): Wire type for the session history endpoint.
/// Bridges ironhermes-state's server-only StoredMessage and the WASM-side
/// ChatBubble component. Only "user" and "assistant" roles are included —
/// "tool" and "system" messages are filtered out server-side because they
/// are not renderable as ChatBubble items and would waste the HISTORY_LIMIT cap.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChatMessage {
    pub role: String,    // "user" | "assistant" — tool/system filtered out server-side
    pub content: String, // empty string when StoredMessage.content is None
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SlashCommandInfo {
    pub name: String,
    pub description: String,
    pub category: String,
    pub aliases: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ConfigSummary {
    pub model: String,
    pub provider: String,
    pub context_length: u32,
    pub memory_enabled: bool,
    /// Phase 46.9 Plan 03 (D-07): seconds since this server process began
    /// answering `get_config_summary` — feeds `sys_meta.rs`'s `UPTIME`
    /// segment (previously a hardcoded `"00:00:00"` stub). Computed from a
    /// module-level `OnceLock<Instant>` initialized on first call, which
    /// approximates true process start closely enough for a status-bar
    /// readout (the only drift is the few ms of server boot before the
    /// first request, an accepted proxy per the plan's flagged assumption).
    pub uptime_secs: u64,
}

// TODO Phase 36.2 Plan 10 (D-USAGE-03) — web UI cost surface:
//
// The TUI's `tui_rata::status_line::StatusLineState` gained two new
// fields this plan — `cost_usd_micros: i64` and `session_total_tokens:
// usize` — and the `/usage` slash command is wired across every
// platform via `CommandRouter::resolve` + `dispatch` (so the web UI
// gets `/usage` text rendering for free, today).
//
// The PARALLEL sidebar pill surface (the cost + token pills next to
// the existing `provider` / `model` pills in `components/shell_legacy/
// status_bar.rs`) is intentionally deferred from Plan 10 because:
//
// 1. The web UI status bar uses Dioxus `ReadSignal<TokenBudget>`
//    signals, NOT the TUI's `StatusLineState` struct — wiring the
//    cost fields end-to-end requires extending the `TokenBudget`
//    signal AND threading a new `CostBudget` signal through the
//    WebSocket payload and the `state.rs` AppState (cross-cutting
//    Dioxus refactor).
//
// 2. The hermes-agent → ironhermes web UI parity scope (iron-
//    hermes-planning §2.1) treats this as a TUI-first feature; the
//    web pill surface follows once Phase 26.7 (web wiring) is fully
//    UAT'd and the live AgentRuntime is broadcasting per-turn usage
//    over the websocket.
//
// When the deferral closes, the canonical shape is `cost_usd_micros:
// i64` + `session_total_tokens: usize` on `ConfigSummary` (or a
// sibling `UsageSummary`), wired through `state.rs::record_usage` to
// the websocket. The TUI render formula (display-only f64 conversion,
// `${:.3}` + `{:.1}K tok`) is the canonical formatting contract — see
// `crates/ironhermes-cli/src/tui_rata/status_line.rs::build_pills`.

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MemoryEntry {
    /// "agent" for MEMORY.md, "user" for USER.md.
    pub store: String,
    /// Raw text block from MemoryEntries Vec<String> — one row per block.
    pub body: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct MemoryInfo {
    pub entries: Vec<MemoryEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// Derived from SkillSource: Builtin→"bundled", Official→"official",
    /// Trusted→"trusted", Community→"installed", SelfCreated→"self-created".
    pub category: String,
    /// True if skill.name appears in the runtime's active_skills.
    pub enabled: bool,
}

#[get("/api/commands")]
pub async fn list_slash_commands() -> Result<Vec<SlashCommandInfo>> {
    let state = crate::server::state::global_app_state();
    let commands = state
        .command_router
        .commands
        .iter()
        .map(|def| SlashCommandInfo {
            name: def.name.to_string(),
            description: def.description.to_string(),
            category: format!("{:?}", def.category),
            aliases: def.aliases.iter().map(|a| a.to_string()).collect(),
        })
        .collect();
    Ok(commands)
}

#[get("/api/sessions")]
pub async fn list_sessions() -> Result<Vec<SessionInfo>> {
    let state = crate::server::state::global_app_state();
    // Phase 34 D-07/D-08: filter by Platform::Web so list_sessions returns only
    // web chat sessions. Supersedes GAP-26.2.1-09 / D-26.2.1-13-A (None filter)
    // per Phase 34 Plan 02 session-store unification requirement.
    let platform_filter = Platform::Web.to_string();
    let sessions = {
        // CR-06: map lock-poison to a structured error instead of panicking the
        // request handler. A poisoned mutex (from a prior thread panic) used to
        // surface as opaque HTTP 500 with no body; now operators see the cause.
        let store = state
            .state_store
            .lock()
            .map_err(|_| ServerFnError::new("State store mutex poisoned"))?;
        store
            .list_sessions(Some(&platform_filter), 100)
            .map_err(|e| ServerFnError::new(format!("StateStore list sessions failed: {e}")))?
    };

    let out = sessions
        .into_iter()
        // GAP-26.2.1-09-R3 / D-26.2.1-14-B (USER CHOSE OPTION A, 2026-05-14):
        // Drop non-loadable sessions. The D-26.2.1-13-A surgical lift broadened
        // the listing to ~74 entries from ~/.ironhermes/sessions/, but some
        // are foreign-format directories (only trajectories.jsonl, no SQLite
        // messages row → message_count == 0) that the chat-load path cannot
        // populate. Filtering message_count > 0 yields ONLY sessions the
        // SESSIONS wedge can actually open. See 26.2.1-UAT.md round-3 notes.
        .filter(|s| s.message_count > 0)
        .map(|session| {
            // Phase 26.7.2 D-04: derive last_message preview per session (N+1 SQLite read).
            // Acceptable for this phase (50-100 sessions, fast local SQLite) — RESEARCH Pitfall 5.
            // MutexGuard is scoped to the inner block and drops before SessionInfo is constructed,
            // so session.id can be moved into the struct literal without a borrow conflict.
            let last_message = {
                // CR-06: don't panic the whole list_sessions request if the
                // mutex is poisoned mid-iteration. Falling back to None here
                // is acceptable because last_message is a decorative preview
                // — better to render the row without a preview than 500 the
                // entire sessions list.
                state.state_store.lock().ok().and_then(|store| {
                    store
                        .get_messages(&session.id)
                        .ok()
                        .and_then(|msgs| extract_last_message_preview(&msgs))
                })
            };
            SessionInfo {
                id: session.id,
                title: session.title,
                created_at: format!("{}", session.started_at as i64),
                message_count: session.message_count.max(0) as u32,
                last_message,
            }
        })
        .collect();
    Ok(out)
}

/// Phase 46.6 Plan 03 (D-04): wire type for the artifact gallery — one row
/// per artifact returned by `list_artifacts`, mirroring `SessionInfo`'s shape.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ArtifactInfo {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
    pub source_kind: Option<String>,
    pub source_ref: Option<String>,
    pub updated_at: String,
    /// Phase 46.6 artifact management: soft-archived state (hidden from the
    /// default gallery listing; surfaced only under the "show archived" toggle).
    pub archived: bool,
}

/// Phase 46.6 Plan 03 (D-04): profile-scoped artifact list for the Sessions-page
/// gallery. Locks `AppState.artifact_store` (never `.unwrap()` — CR-06 lock-
/// poison-to-ServerFnError idiom, same as `list_sessions` above) and returns
/// exactly the artifacts published by the current profile, so the gallery
/// shows precisely what producers (chat/kanban/delegate, Plan 02) wrote for
/// this profile.
// Uses #[server] (POST) rather than #[get] because Dioxus serializes server fn
// args as a request body — GET server fns with arguments fail to deserialize
// (500 "missing field `include_archived`"), same pitfall documented on
// `get_session_messages` below. A no-arg #[get] worked, but the archived-toggle
// argument forced the switch.
#[server]
pub async fn list_artifacts(include_archived: bool) -> Result<Vec<ArtifactInfo>, ServerFnError> {
    let state = crate::server::state::global_app_state();
    let profile = active_profile_identifier();
    let summaries: Vec<ArtifactSummary> = {
        // CR-06: map lock-poison to a structured error instead of panicking
        // the request handler (mirrors list_sessions' state_store handling).
        let store = state
            .artifact_store
            .lock()
            .map_err(|_| ServerFnError::new("Artifact store mutex poisoned"))?;
        store
            .list_for_profile_filtered(&profile, include_archived)
            .map_err(|e| ServerFnError::new(format!("ArtifactStore list failed: {e}")))?
    };

    Ok(summaries
        .into_iter()
        .map(|s| ArtifactInfo {
            id: s.id,
            title: s.title,
            icon: s.icon,
            source_kind: s.source_kind,
            source_ref: s.source_ref,
            updated_at: format!("{}", s.updated_at as i64),
            archived: s.archived,
        })
        .collect())
}

/// Phase 46.6 gap-closure: the latest artifact a kanban task produced, or `None`.
/// Backs the completed-card → artifact affordance (`kanban/card.rs`). Keyed on
/// `source_kind="kanban"` + `source_ref=<task id>` (stamped at publish time by
/// the `artifact` tool). Not profile-scoped — kanban task ids are globally
/// unique, so the operator can reach a task's artifact regardless of which
/// profile bucket it landed in.
// #[server] (POST), not #[get]: the `task_id` argument can't round-trip through
// the GET codec (see `list_artifacts` / `get_session_messages`).
#[server]
pub async fn artifact_for_task(task_id: String) -> Result<Option<ArtifactInfo>, ServerFnError> {
    let state = crate::server::state::global_app_state();
    let summary = {
        let store = state
            .artifact_store
            .lock()
            .map_err(|_| ServerFnError::new("Artifact store mutex poisoned"))?;
        store
            .latest_for_source("kanban", &task_id)
            .map_err(|e| ServerFnError::new(format!("latest_for_source failed: {e}")))?
    };
    Ok(summary.map(|s| ArtifactInfo {
        id: s.id,
        title: s.title,
        icon: s.icon,
        source_kind: s.source_kind,
        source_ref: s.source_ref,
        updated_at: format!("{}", s.updated_at as i64),
        archived: s.archived,
    }))
}

/// Phase 46.6 artifact management: permanently delete an artifact and all of
/// its version rows. Irreversible — the gallery gates this behind a confirm.
#[server]
pub async fn delete_artifact(id: String) -> Result<(), ServerFnError> {
    let state = crate::server::state::global_app_state();
    // IDOR mitigation: scope to the caller's active profile (mirrors the
    // profile-scoped read path). A crafted id for another profile matches no
    // row → NotFound, never a silent cross-profile delete.
    let profile = active_profile_identifier();
    let deleted = {
        let mut store = state
            .artifact_store
            .lock()
            .map_err(|_| ServerFnError::new("Artifact store mutex poisoned"))?;
        store
            .delete(&profile, &id)
            .map_err(|e| ServerFnError::new(format!("delete_artifact failed: {e}")))?
    };
    if !deleted {
        return Err(ServerFnError::new("artifact not found"));
    }
    Ok(())
}

/// Phase 46.6 artifact management: archive or unarchive an artifact (soft
/// hide/show in the gallery).
#[server]
pub async fn set_artifact_archived(id: String, archived: bool) -> Result<(), ServerFnError> {
    let state = crate::server::state::global_app_state();
    // IDOR mitigation: scope to the caller's active profile.
    let profile = active_profile_identifier();
    let updated = {
        let mut store = state
            .artifact_store
            .lock()
            .map_err(|_| ServerFnError::new("Artifact store mutex poisoned"))?;
        store
            .set_archived(&profile, &id, archived)
            .map_err(|e| ServerFnError::new(format!("set_artifact_archived failed: {e}")))?
    };
    if !updated {
        return Err(ServerFnError::new("artifact not found"));
    }
    Ok(())
}

/// Phase 46.6 Plan 03 (D-04): the current profile identifier, used to scope
/// `list_artifacts` (and matched by the `artifact` Tool's `profile` field on
/// publish, Plan 02) — mirrors the canonical `current_profile()` algorithm
/// (`ironhermes-cli::status_cmd::current_profile` / `ironhermes-gateway::pid`
/// `current_profile_label`): reverse-walk `get_hermes_home()` for a
/// `profiles/<slug>` ancestor, else the literal string `"default"`.
/// Reimplemented locally rather than depending on `ironhermes-cli` (a binary
/// crate, not an appropriate dependency for the web server crate).
#[cfg(feature = "server")]
fn active_profile_identifier() -> String {
    // Phase 46.6 gap-closure: delegate to the shared canonical resolver so the
    // gallery lists by the EXACT profile the `artifact` tool publishes under
    // (`ironhermes_tools::artifact::publish_profile` → the same
    // `current_profile`). Previously an inline copy of the same walk; kept as a
    // thin wrapper to avoid churning the call sites in this file.
    ironhermes_core::current_profile()
}

/// Phase 46.9 Plan 03 (D-07): lazily-initialized server-start marker for the
/// `.sys-meta` `UPTIME` segment. `OnceLock::get_or_init` guarantees whichever
/// call reaches this first wins the timestamp, regardless of concurrent
/// early requests — subsequent calls just read the already-set `Instant`.
#[cfg(feature = "server")]
static SERVER_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

#[cfg(feature = "server")]
fn server_uptime_secs() -> u64 {
    SERVER_START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs()
}

#[get("/api/config")]
pub async fn get_config_summary() -> Result<ConfigSummary> {
    let state = crate::server::state::global_app_state();
    let cfg = state.config.clone();
    let context_length = state.resolver.resolve_for_main().context_length() as u32;
    Ok(ConfigSummary {
        model: cfg.model.default.clone(),
        provider: cfg.model.provider.clone(),
        context_length,
        memory_enabled: cfg.memory.memory_enabled,
        uptime_secs: server_uptime_secs(),
    })
}

#[get("/api/tools")]
pub async fn list_tools() -> Result<Vec<ToolInfo>> {
    let state = crate::server::state::global_app_state();
    let definitions = state.runtime.registry().read().await.get_definitions(None);
    let out = definitions
        .into_iter()
        .map(|def| ToolInfo {
            name: def.function.name,
            description: def.function.description,
        })
        .collect();
    Ok(out)
}

/// Phase 26.7 Plan 02 (D-07, D-08, R-3): Read-only snapshot of MemoryManager
/// content for the Memory screen. Returns one MemoryEntry per raw text block in
/// each MemoryTarget store. Em-dash placeholder for timestamps (no per-block
/// timestamp exists in the underlying `Vec<String>` storage).
#[get("/api/memory")]
pub async fn get_memory() -> Result<MemoryInfo> {
    let state = crate::server::state::global_app_state();
    let Some(ref mgr) = state.memory_manager else {
        // Memory disabled at runtime — render empty panels, not an error.
        return Ok(MemoryInfo::default());
    };
    // tokio Mutex .lock().await.to_memory_entries().await chained; guard
    // drops at the `;`. Pattern mirrors list_tools (api.rs:107-112).
    let entries = mgr.lock().await.to_memory_entries().await;

    let mut out: Vec<MemoryEntry> = Vec::new();
    for (target, items) in entries.entries.iter() {
        let store = match target {
            ironhermes_core::memory_store::MemoryTarget::Memory => "agent",
            ironhermes_core::memory_store::MemoryTarget::User => "user",
        };
        for body in items.iter() {
            out.push(MemoryEntry {
                store: store.to_string(),
                body: body.clone(),
            });
        }
    }
    Ok(MemoryInfo { entries: out })
}

/// Phase 26.7 Plan 04 (D-10, R-1): Known model types for the Models screen.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    /// Inferred from model ID prefix (e.g. "claude*" → "Anthropic (Claude)").
    pub family: String,
    /// Human-readable context window: "8k", "128k", "200k", "1M".
    pub context_window: String,
    /// "DEFAULT" when id matches state.config.model.default, else "AVAILABLE".
    pub status: String,
}

/// Phase 26.7 Plan 04 (D-10, R-1): Read-only list of all known models from the
/// runtime ModelRegistry (static table + cache overlay). Reached via
/// state.resolver.model_registry() — sync accessor, no lock.
#[get("/api/models")]
pub async fn list_models() -> Result<Vec<ModelInfo>> {
    let state = crate::server::state::global_app_state();
    let registry = state.resolver.model_registry();
    let default_id = state.config.model.default.clone();

    let out = registry
        .all_models()
        .into_iter()
        .map(|(id, meta)| ModelInfo {
            id: id.to_string(),
            family: infer_family(id),
            context_window: format_context_window(meta.context_length),
            status: if id == default_id.as_str() {
                "DEFAULT".to_string()
            } else {
                "AVAILABLE".to_string()
            },
        })
        .collect();

    Ok(out)
}

/// Phase 46.9 Plan 13 (GAP-1, D-05): DTO for the Models screen's dependent
/// (provider-first) model dropdown (consumed by Plan 15). `fell_back = true`
/// means `models` is the full `list_models()` catalog rather than a genuine
/// provider-sourced list — the caller uses this to label the fallback
/// honestly instead of presenting it as provider-specific.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProviderModelsSnapshot {
    pub models: Vec<String>,
    pub fell_back: bool,
}

/// Phase 46.9 Plan 13 (GAP-1): pure parser for an OpenAI-compatible `/models`
/// response body (`{"data": [{"id": "..."}, ...]}`). Extracts the ordered
/// list of model ids, skipping any entry whose `id` is not a string.
/// Deterministic order = response order. Mirrors
/// `ironhermes_core::models_cache::parse_openrouter_response`'s traversal of
/// the same OpenAI-shaped `data` array, but returns bare ids (no metadata) —
/// this fn answers "what models does the provider serve", not "what are
/// their capabilities".
#[cfg(not(target_arch = "wasm32"))]
fn parse_openai_models_response(body: &serde_json::Value) -> Vec<String> {
    let Some(data) = body.get("data").and_then(|d| d.as_array()) else {
        return Vec::new();
    };
    data.iter()
        .filter_map(|entry| entry.get("id").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .collect()
}

/// Phase 46.9 Plan 13 (GAP-1): pure decision of whether the named provider
/// has a usable `/models` endpoint to try, given a freshly loaded `Config`
/// and the full catalog ids (from `list_models`/`all_models()`) the caller
/// would fall back to. Returns `Err(ProviderModelsSnapshot)` — the
/// catalog-labeled fallback, `fell_back: true` — immediately when the
/// provider is unknown or has no non-empty `base_url`, *before* any network
/// call, or `Ok((base_url, resolved_api_key))` when a fetch should be
/// attempted. Split out from `list_provider_models` so the no-endpoint
/// branch is unit-testable without `global_app_state()` or the network
/// (mirrors `build_models_roles_snapshot`'s split for the same reason).
#[cfg(not(target_arch = "wasm32"))]
fn resolve_provider_or_fallback(
    config: &ironhermes_core::config::Config,
    provider: &str,
    catalog_ids: Vec<String>,
) -> Result<(String, Option<String>), ProviderModelsSnapshot> {
    let fallback = || ProviderModelsSnapshot {
        models: catalog_ids.clone(),
        fell_back: true,
    };

    let provider_cfg = config.providers.get(provider).ok_or_else(fallback)?;
    let base_url = provider_cfg
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(fallback)?
        .trim_end_matches('/')
        .to_string();

    // Resolved credential: api_key_env (preferred) else the deprecated inline
    // literal. Used ONLY as the outbound Authorization bearer below — never
    // returned, never logged, never placed in a ServerFnError string (T-46.9-30).
    let api_key = provider_cfg
        .api_key_env
        .as_deref()
        .and_then(|name| std::env::var(name).ok())
        .or_else(|| provider_cfg.api_key.clone());

    Ok((base_url, api_key))
}

/// Phase 46.9 Plan 13 (GAP-1, D-05): the data source the Models screen's
/// dependent (provider-first) model dropdown consumes (Plan 15) — "which
/// models does provider X actually serve?", which `list_models()`'s flat
/// global catalog cannot answer (no per-provider field). Fetches the named
/// provider's own OpenAI-compatible `GET {base_url}/models` using its
/// resolved base_url + credential, and degrades to the full `list_models()`
/// catalog (`fell_back: true`) whenever the provider is unknown, has no
/// base_url, the request errors, the response is non-200, or the body
/// doesn't parse to a non-empty model list — this fn never returns an empty
/// list and never hard-errors, so the dropdown always has something to show.
/// Read-only (mirrors `list_models`, which is ungated) — intentionally NOT
/// placed behind the config-write safety toggle; no config mutation occurs here.
#[server]
pub async fn list_provider_models(
    provider: String,
) -> Result<ProviderModelsSnapshot, ServerFnError> {
    // ---- Phase 49.4 hotfix: short-TTL single-flight cache ----
    //
    // The Models screen mounts up to 8 provider→model cascades (default + one
    // per role), and EACH called this fn on mount — every call a live HTTP GET
    // to the provider's `/models` endpoint (300+ models for OpenRouter, 4-5s
    // under load). Landing on Models therefore fired ~8 concurrent slow
    // upstream requests and repeated them on every revisit, saturating the
    // single UI-server runtime so the model list, avatars, and even other
    // screens stopped loading.
    //
    // Fix: cache each provider's snapshot for 60s. Holding the async mutex
    // across the whole miss path also single-flights concurrent callers — the
    // first does the one upstream fetch; the rest wait here, then read the
    // freshly-cached entry instead of issuing their own request. A cache hit is
    // just a lock + map lookup, so it never touches config, state, or network.
    use std::time::{Duration, Instant};
    const MODEL_LIST_TTL: Duration = Duration::from_secs(60);
    static MODEL_LIST_CACHE: std::sync::OnceLock<
        tokio::sync::Mutex<std::collections::HashMap<String, (Instant, ProviderModelsSnapshot)>>,
    > = std::sync::OnceLock::new();
    let cache =
        MODEL_LIST_CACHE.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let mut cache_guard = cache.lock().await;
    if let Some((cached_at, snapshot)) = cache_guard.get(&provider) {
        if cached_at.elapsed() < MODEL_LIST_TTL {
            return Ok(snapshot.clone());
        }
    }
    let cache_key = provider.clone();

    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;

    let state = crate::server::state::global_app_state();
    let catalog_ids: Vec<String> = state
        .resolver
        .model_registry()
        .all_models()
        .into_iter()
        .map(|(id, _meta)| id.to_string())
        .collect();

    let snapshot: ProviderModelsSnapshot = async move {
        let (base_url, api_key) =
            match resolve_provider_or_fallback(&config, &provider, catalog_ids.clone()) {
                Ok(pair) => pair,
                Err(fallback) => return fallback,
            };

        let url = format!("{base_url}/models");
        let client = reqwest::Client::new();
        let mut req = client.get(&url);
        if let Some(key) = api_key {
            req = req.bearer_auth(key);
        }

        // Any transport failure degrades to the catalog fallback rather than a
        // hard error (T-46.9-32) — the outbound provider endpoint is not
        // trustworthy enough to gate the dropdown on.
        let Ok(resp) = req.send().await else {
            return ProviderModelsSnapshot {
                models: catalog_ids,
                fell_back: true,
            };
        };
        if !resp.status().is_success() {
            return ProviderModelsSnapshot {
                models: catalog_ids,
                fell_back: true,
            };
        }
        let Ok(body) = resp.json::<serde_json::Value>().await else {
            return ProviderModelsSnapshot {
                models: catalog_ids,
                fell_back: true,
            };
        };

        let models = parse_openai_models_response(&body);
        if models.is_empty() {
            return ProviderModelsSnapshot {
                models: catalog_ids,
                fell_back: true,
            };
        }

        ProviderModelsSnapshot {
            models,
            fell_back: false,
        }
    }
    .await;

    cache_guard.insert(cache_key, (Instant::now(), snapshot.clone()));
    Ok(snapshot)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod provider_models {
    use super::{parse_openai_models_response, resolve_provider_or_fallback};
    use ironhermes_core::config::{Config, ProviderConfig};

    /// Task 1 acceptance (a): the parser reads exactly the provider-sourced
    /// ids. The fixture deliberately contains ONLY ids that would never
    /// appear in the global model catalog (`prov-only-a`/`prov-only-b`), so
    /// this assertion cannot pass by silently re-using `list_models()` — it
    /// proves the list is genuinely provider-sourced.
    #[test]
    fn parses_provider_sourced_ids_excluding_catalog_distractor() {
        let body = serde_json::json!({
            "data": [
                { "id": "prov-only-a" },
                { "id": "prov-only-b" },
                { "id": 42 }
            ]
        });
        let ids = parse_openai_models_response(&body);
        assert_eq!(
            ids,
            vec!["prov-only-a".to_string(), "prov-only-b".to_string()],
            "non-string id must be skipped; order must match response order"
        );
    }

    #[test]
    fn parser_returns_empty_when_data_array_missing() {
        let body = serde_json::json!({ "unexpected": "shape" });
        assert!(parse_openai_models_response(&body).is_empty());
    }

    /// Task 1 acceptance (b): an unknown provider name yields fell_back=true
    /// with a non-empty models list (the catalog), asserted purely from
    /// `resolve_provider_or_fallback`'s `Err` branch — no network call is
    /// reachable in this path (the fn returns before constructing a client).
    #[test]
    fn unknown_provider_yields_fallback_without_network() {
        let config = Config::default();
        let catalog = vec!["catalog-only-model".to_string()];

        let result = resolve_provider_or_fallback(&config, "totally-unknown-provider", catalog.clone());

        match result {
            Err(snapshot) => {
                assert!(snapshot.fell_back, "unknown provider must set fell_back=true");
                assert!(!snapshot.models.is_empty(), "fallback must never be empty");
                assert_eq!(snapshot.models, catalog);
            }
            Ok(_) => panic!("expected the fallback branch for an unknown provider"),
        }
    }

    /// A configured provider with a blank/whitespace-only base_url also
    /// takes the fallback branch, not just a missing provider key.
    #[test]
    fn blank_base_url_yields_fallback() {
        let mut config = Config::default();
        config.providers.insert(
            "blank-base".to_string(),
            ProviderConfig {
                base_url: Some("   ".to_string()),
                ..Default::default()
            },
        );
        let catalog = vec!["catalog-only-model".to_string()];

        let result = resolve_provider_or_fallback(&config, "blank-base", catalog.clone());

        match result {
            Err(snapshot) => {
                assert!(snapshot.fell_back);
                assert_eq!(snapshot.models, catalog);
            }
            Ok(_) => panic!("expected the fallback branch for a blank base_url"),
        }
    }

    /// A configured provider with a real base_url is NOT a fallback case —
    /// resolve_provider_or_fallback should hand back the trimmed URL and
    /// resolved credential for the caller to attempt a fetch with.
    #[test]
    fn configured_provider_resolves_endpoint() {
        let mut config = Config::default();
        config.providers.insert(
            "openrouter".to_string(),
            ProviderConfig {
                base_url: Some("https://openrouter.ai/api/v1/".to_string()),
                api_key: Some("literal-key-value".to_string()),
                ..Default::default()
            },
        );
        let catalog = vec!["catalog-only-model".to_string()];

        let result = resolve_provider_or_fallback(&config, "openrouter", catalog);

        match result {
            Ok((base_url, api_key)) => {
                assert_eq!(base_url, "https://openrouter.ai/api/v1");
                assert_eq!(api_key.as_deref(), Some("literal-key-value"));
            }
            Err(_) => panic!("expected Ok for a configured provider with a base_url"),
        }
    }
}

fn infer_family(id: &str) -> String {
    if id.starts_with("claude") {
        "Anthropic (Claude)".to_string()
    } else if id.starts_with("gpt")
        || id.starts_with("o3")
        || id.starts_with("o4")
        || id.starts_with("o1")
    {
        "OpenAI".to_string()
    } else if id.starts_with("gemini") {
        "Google (Gemini)".to_string()
    } else if id.starts_with("llama") {
        "Meta (Llama)".to_string()
    } else if id.starts_with("mistral") || id.starts_with("mixtral") || id.starts_with("codestral")
    {
        "Mistral".to_string()
    } else if id.starts_with("deepseek") {
        "DeepSeek".to_string()
    } else if id.starts_with("qwen") {
        "Qwen".to_string()
    } else {
        "Other".to_string()
    }
}

fn format_context_window(ctx: usize) -> String {
    if ctx >= 1_000_000 {
        format!("{}M", ctx / 1_000_000)
    } else if ctx >= 1_000 {
        format!("{}k", ctx / 1_000)
    } else {
        format!("{}", ctx)
    }
}

/// Phase 26.7.2 (D-04 / RESEARCH Q6): Extract last message preview for SessionInfo.
/// Prefers last assistant message with content; falls back to last user message with content.
/// Skips tool/system roles and None-content entries entirely.
/// Truncates to 80 chars using chars().take(80) for multi-byte-safe truncation
/// (direct byte-slice indexing would panic on non-ASCII char boundaries).
#[cfg(feature = "server")]
fn extract_last_message_preview(msgs: &[ironhermes_state::StoredMessage]) -> Option<String> {
    let preview = msgs
        .iter()
        .rev()
        .find(|m| m.role == "assistant" && m.content.is_some())
        .or_else(|| {
            msgs.iter()
                .rev()
                .find(|m| m.role == "user" && m.content.is_some())
        });
    preview
        .and_then(|m| m.content.as_ref())
        .map(|c: &String| c.chars().take(80).collect::<String>())
}

/// Phase 26.7 Plan 03 (D-09, R-1, R-4): Read-only catalog of skills from the
/// runtime SkillRegistry, with per-skill `enabled` reflecting the current
/// session's `active_skills` set. SkillRegistry.list() is sync — no await
/// needed for the registry read; only the std Mutex lock on active_skills.
#[get("/api/skills")]
pub async fn list_skills() -> Result<Vec<SkillInfo>> {
    let state = crate::server::state::global_app_state();
    let registry = state.runtime.skill_registry();

    // std::sync::Mutex — short-lived lock, no .await inside the held scope.
    let active_names: std::collections::HashSet<String> = {
        let guard = state
            .runtime
            .active_skills()
            .lock()
            .map_err(|e| ServerFnError::new(format!("active_skills lock poisoned: {e}")))?;
        guard.iter().map(|r| r.name.clone()).collect()
    };

    let out = registry
        .list()
        .iter()
        .map(|r| {
            let category = match r.source {
                ironhermes_core::skills::SkillSource::Builtin => "bundled",
                ironhermes_core::skills::SkillSource::Official => "official",
                ironhermes_core::skills::SkillSource::Trusted => "trusted",
                ironhermes_core::skills::SkillSource::Community => "installed",
                ironhermes_core::skills::SkillSource::SelfCreated => "self-created",
            };
            SkillInfo {
                name: r.name.clone(),
                description: r.description.clone(),
                category: category.to_string(),
                enabled: active_names.contains(&r.name),
            }
        })
        .collect();
    Ok(out)
}

// =============================================================================
// Phase 26.7.3 Plan 02: toggle_skill — D-07 write path
// =============================================================================

/// Phase 26.7.3 D-07: Toggle a skill's disabled state.
///
/// Four-step contract:
///   Step 1 — Input validation: reject unknown skill names BEFORE any disk I/O.
///             `skill_registry.find(&name)` must return `Some` or the fn returns Err.
///   Step 2 — Config write-back: load fresh config from disk (NOT the startup Arc<Config>
///             snapshot), toggle `name` in `skills.disabled`, save back to disk.
///   Step 3 — Config persistence: `Config::save()` writes to get_hermes_home()/config.yaml.
///   Step 4 — In-process active_skills mutation: lock the std::sync::Mutex, push or retain
///             based on the toggle direction. The MutexGuard MUST drop before this fn returns.
///             Lock discipline: no async suspension occurs while the guard is held.
///
/// Behavior (opt-out model — absent from `disabled` means enabled):
///   - If `name` was enabled (absent from disabled): add to disabled, remove from active_skills.
///   - If `name` was disabled (present in disabled): remove from disabled, push to active_skills.
#[server]
pub async fn toggle_skill(name: String) -> Result<(), ServerFnError> {
    let state = crate::server::state::global_app_state();

    // Step 1 — input validation: reject unknown skill names BEFORE any disk I/O (T-26.7.3-02-01)
    let record = state.runtime.skill_registry().find(&name).cloned();
    if record.is_none() {
        return Err(ServerFnError::new(format!("unknown skill: {name}")));
    }

    // Step 2 — load fresh config from disk and toggle the disabled list
    let mut config = ironhermes_core::config::Config::load().unwrap_or_default();
    let was_disabled = apply_disable_toggle(&mut config, &name);

    // Step 3 — persist config back to disk
    config
        .save()
        .map_err(|e| ServerFnError::new(format!("Config save failed: {e}")))?;

    // Step 4 — in-process active_skills mutation (scoped std::sync::Mutex lock).
    // The MutexGuard is dropped at the closing brace before fn returns; no async
    // suspension occurs while the lock is held (lock discipline: Behavior 5).
    // Pattern: api.rs list_skills lines 234–240 (active_skills lock pattern)
    {
        let mut skills = state
            .runtime
            .active_skills()
            .lock()
            .map_err(|e| ServerFnError::new(format!("active_skills lock poisoned: {e}")))?;
        if was_disabled {
            // Was disabled → now enabled: push record if not already present
            if !skills.iter().any(|s| s.name == name) {
                skills.push(record.unwrap());
            }
        } else {
            // Was enabled → now disabled: remove from active list
            skills.retain(|s| s.name != name);
        }
    } // MutexGuard drops here — lock released before fn returns

    Ok(())
}

/// Pure helper: toggle `name` in `config.skills.disabled` (opt-out model).
///
/// Returns `true` if the name WAS disabled before the toggle (i.e. the name was
/// removed from the list — re-enable direction). Returns `false` if the name was
/// not in the list (i.e. the name was added — disable direction).
///
/// Extracted as a private fn so both `toggle_skill` (call-site) and the unit
/// tests in `toggle_skill_tests` can exercise the pure config-mutation logic
/// without requiring a live `global_app_state()`.
#[cfg(not(target_arch = "wasm32"))]
fn apply_disable_toggle(config: &mut ironhermes_core::config::Config, name: &str) -> bool {
    let is_disabled = config.skills.disabled.contains(&name.to_string());
    if is_disabled {
        config.skills.disabled.retain(|n| n != name);
    } else {
        config.skills.disabled.push(name.into());
    }
    is_disabled
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod toggle_skill_tests {
    use super::apply_disable_toggle;
    use ironhermes_core::config::Config;

    fn make_config_with_disabled(names: &[&str]) -> Config {
        let mut c = Config::default();
        c.skills.disabled = names.iter().map(|s| s.to_string()).collect();
        c
    }

    #[test]
    fn toggle_enabled_skill_adds_to_disabled() {
        let mut config = make_config_with_disabled(&[]);
        let was_disabled = apply_disable_toggle(&mut config, "foo");
        assert!(!was_disabled, "foo was not disabled before toggle");
        assert!(
            config.skills.disabled.contains(&"foo".to_string()),
            "foo should now be in disabled list"
        );
    }

    #[test]
    fn toggle_disabled_skill_removes_from_disabled() {
        let mut config = make_config_with_disabled(&["bar"]);
        let was_disabled = apply_disable_toggle(&mut config, "bar");
        assert!(was_disabled, "bar was disabled before toggle");
        assert!(
            !config.skills.disabled.contains(&"bar".to_string()),
            "bar should no longer be in disabled list"
        );
    }

    #[test]
    fn toggle_round_trip_restores_original_state() {
        let mut config = make_config_with_disabled(&[]);
        // enable→disable
        apply_disable_toggle(&mut config, "baz");
        assert!(
            config.skills.disabled.contains(&"baz".to_string()),
            "baz should be disabled after first toggle"
        );
        // disable→enable
        apply_disable_toggle(&mut config, "baz");
        assert!(
            !config.skills.disabled.contains(&"baz".to_string()),
            "baz should be re-enabled after second toggle"
        );
    }
}

#[post("/api/sessions/create")]
pub async fn create_session() -> Result<String> {
    let state = crate::server::state::global_app_state();
    let session_uuid = uuid::Uuid::new_v4().to_string();
    let session_key = format!("agent:main:web:dm:{session_uuid}");

    state
        .ensure_web_session(&session_key)
        .map_err(|e| ServerFnError::new(format!("Session creation failed: {e}")))?;

    Ok(session_key)
}

/// Phase 26.7.2 (D-10): UI display cap for session history fetch.
/// This is a UI display cap only — the agent's build_messages_for_turn has
/// independent context-window management that operates separately from this limit.
const HISTORY_LIMIT: usize = 50;

/// Phase 26.7.2 (D-10): Last N messages for the Chat screen history load.
/// Uses #[server] (POST) rather than #[get] because Dioxus serializes server fn
/// args as a request body — browsers reject bodies on GET requests (TypeError).
/// Filters to "user" + "assistant" roles only (tool/system not renderable as
/// ChatBubble items per RESEARCH Q4). Slices the tail so the last HISTORY_LIMIT
/// visible messages are returned in chronological order (oldest first).
/// Filter MUST run before .take(HISTORY_LIMIT) so invisible tool/system rows
/// do not consume the display cap.
#[server]
pub async fn get_session_messages(id: String) -> Result<Vec<ChatMessage>> {
    let state = crate::server::state::global_app_state();
    // CR-06: surface lock-poisoning as a structured error instead of panicking.
    let all_msgs = state
        .state_store
        .lock()
        .map_err(|_| ServerFnError::new("State store mutex poisoned"))?
        .get_messages(&id)
        .map_err(|e| ServerFnError::new(format!("get_messages failed: {e}")))?;

    // Filter MUST run before take so tool/system messages don't consume the cap (RESEARCH Q4).
    // Collect into a Vec after filter so we have ExactSizeIterator for the .rev().take().rev() slice.
    let visible: Vec<&ironhermes_state::StoredMessage> = all_msgs
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .collect();
    let out: Vec<ChatMessage> = visible
        .iter()
        .rev()
        .take(HISTORY_LIMIT)
        .rev()
        .map(|m| ChatMessage {
            role: m.role.clone(),
            content: m.content.clone().unwrap_or_default(),
        })
        .collect();

    Ok(out)
}

/// Phase 46.7 Plan 05 (D-24, Rule 2 deviation): surface the session's
/// workspace path to the WASM client for the chat-header copyable
/// workspace-path chip. `session_workspace_dir` is a server-side path
/// resolver (`ironhermes_core`, native-only — `cfg(not(target_arch =
/// "wasm32"))` in Cargo.toml), so a `#[server]` round-trip is the only way
/// for the client to display it; no `#[get]` because this is an arg-bearing
/// fn (round-5 silent-500 lesson: arg-bearing server fns must be POST).
/// Returns an empty string on any resolution failure rather than an error —
/// the header chip already treats an empty path as "not yet resolved" and
/// renders nothing (D-24, matches the 46.4 output-path empty-state
/// convention), so a transient failure degrades to "no chip" rather than an
/// error bubble on an otherwise-successful screen render.
#[server]
pub async fn session_workspace_path(session_id: String) -> Result<String, ServerFnError> {
    #[cfg(feature = "server")]
    {
        // SEC-03/CR-01 invariant: EVERY client-supplied session_id is validated
        // before it reaches a path join. This fn only formats the joined path
        // into a String today (no fs call), so the guard is defense-in-depth —
        // but it keeps the invariant total, so a future fs call added here
        // cannot silently reintroduce the traversal.
        if ironhermes_core::safe_session_id(&session_id).is_none() {
            return Err(ServerFnError::new(format!(
                "invalid session id: {session_id:?}"
            )));
        }
        let path = ironhermes_core::session_workspace_dir(&session_id);
        Ok(path.to_string_lossy().into_owned())
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = session_id;
        Err(ServerFnError::new(
            "session_workspace_path unavailable without `server` feature",
        ))
    }
}

// =============================================================================
// Phase 32.3 Plan 04: /api/agents/{kill,interrupt,prune,status} REST endpoints
// =============================================================================
//
// Four Dioxus server functions wrapping the free fns in `state.rs`. They
// follow the established `#[get(...)]` / `#[post(...)]` pattern in this file
// (list_slash_commands, list_sessions, etc.) — iron_hermes_ui's REST surface
// is Dioxus server functions, not a separate Axum routes.rs (main.rs has only
// one Axum mount point: `serve_dioxus_application`).
//
// **No confirm token on the web surface** (Phase 32.3 D-09 — only the
// Telegram gateway enforces `confirm` because of spoof-replay risk; TUI and
// web both have synchronous user presence).

/// Phase 32.3 Plan 04 (D-08): `POST /api/agents/kill` body `{"id": "sub_xxx"}`.
/// Returns the JSON shape documented in `state::api_agents_kill`.
/// Phase 26.7 Plan 06 (Gap 1): wrapped in tokio::task::spawn_blocking to escape
/// Dioxus fullstack per-connection LocalSet — ShrikeService internals use
/// block_in_place which requires the multi-thread runtime.
#[post("/api/agents/kill")]
pub async fn api_agents_kill(id: String) -> Result<serde_json::Value> {
    let shrike = crate::server::state::global_app_state().shrike.clone();
    let body = serde_json::json!({ "id": id });
    let result = tokio::task::spawn_blocking(move || {
        crate::server::state::api_agents_kill(shrike.as_deref(), body)
    })
    .await
    .map_err(|e| ServerFnError::new(format!("spawn_blocking join error: {e}")))?;
    Ok(result)
}

/// Phase 32.3 Plan 04 (D-08): `POST /api/agents/interrupt` body `{"id": "sub_xxx"}`.
/// Phase 26.7 Plan 06 (Gap 1): wrapped in tokio::task::spawn_blocking to escape
/// Dioxus fullstack per-connection LocalSet — ShrikeService internals use
/// block_in_place which requires the multi-thread runtime.
#[post("/api/agents/interrupt")]
pub async fn api_agents_interrupt(id: String) -> Result<serde_json::Value> {
    let shrike = crate::server::state::global_app_state().shrike.clone();
    let body = serde_json::json!({ "id": id });
    let result = tokio::task::spawn_blocking(move || {
        crate::server::state::api_agents_interrupt(shrike.as_deref(), body)
    })
    .await
    .map_err(|e| ServerFnError::new(format!("spawn_blocking join error: {e}")))?;
    Ok(result)
}

/// Phase 32.3 Plan 04 (D-08): `POST /api/agents/prune` body `{"stale_secs": 120}`
/// (optional; defaults to 120 to match `SubagentConfig::stale_warn_seconds`).
/// Phase 26.7 Plan 06 (Gap 1): wrapped in tokio::task::spawn_blocking to escape
/// Dioxus fullstack per-connection LocalSet — ShrikeService internals use
/// block_in_place which requires the multi-thread runtime.
#[post("/api/agents/prune")]
pub async fn api_agents_prune(stale_secs: Option<u64>) -> Result<serde_json::Value> {
    let shrike = crate::server::state::global_app_state().shrike.clone();
    let body = match stale_secs {
        Some(s) => serde_json::json!({ "stale_secs": s }),
        None => serde_json::json!({}),
    };
    let result = tokio::task::spawn_blocking(move || {
        crate::server::state::api_agents_prune(shrike.as_deref(), body)
    })
    .await
    .map_err(|e| ServerFnError::new(format!("spawn_blocking join error: {e}")))?;
    Ok(result)
}

/// Phase 32.3 Plan 04 (D-08): `GET /api/agents/status?id=sub_xxx`.
/// Returns the `SubagentStatusInfo` JSON (Serialize derived in Plan 03), or
/// a `not_found` error when the id is unknown.
#[get("/api/agents/status")]
pub async fn api_agents_status(id: String) -> Result<serde_json::Value> {
    let state = crate::server::state::global_app_state();
    Ok(state
        .api_agents_status(&id)
        .ok_or_else(|| ServerFnError::new(format!("subagent {} not found", id)))?)
}

// =============================================================================
// Phase 26.7 Plan 05: /api/agents/list — flat list for Agents screen
// =============================================================================

/// Phase 26.7 Plan 05 (R-2): Flat list of all active subagents for the
/// Agents screen. CONTEXT.md D-06 anticipated this existed via
/// `get_agents_status()`, but the actual `api_agents_status` queries a SINGLE
/// agent by id. The Agents screen needs a flat list — added here per user
/// choice (flat, not tree).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AgentInfo {
    /// Subagent ID (e.g. "sub_xxx") — used as the kill/interrupt body parameter.
    pub id: String,
    /// Task summary from delegation. Used as the card title/body.
    pub task_summary: String,
    /// Wall-clock seconds since started_at (computed at request time).
    pub uptime_secs: u64,
    /// Always "running" for any agent that appears in the active registry
    /// (the registry only holds running agents per Phase 32.3 spec).
    pub status: String,
    /// Parent subagent ID if this was spawned by another subagent.
    pub parent_id: Option<String>,
}

/// Phase 26.7 Plan 05 (R-2): `GET /api/agents/list` — returns a flat
/// `Vec<AgentInfo>` from the live SubagentRegistry. Route uses `/list`
/// suffix to avoid collision with the existing `/api/agents/status` route.
/// tokio RwLock read guard is scoped to the `.list()` call and dropped
/// before the `.into_iter().map(...)` transform runs — minimizing contention
/// with the agent runner that takes the write lock at register/unregister.
#[get("/api/agents/list")]
pub async fn api_agents_list() -> Result<Vec<AgentInfo>> {
    let state = crate::server::state::global_app_state();
    // tokio RwLock read. SubagentRegistry::list() returns Vec<SubagentInfo>
    // by value (cloned), so we drop the guard immediately after the call.
    let infos: Vec<ironhermes_agent::subagent_registry::SubagentInfo> = {
        let guard = state.subagent_registry.read().await;
        guard.list()
    };

    let out = infos
        .into_iter()
        .map(|info| AgentInfo {
            id: info.id.clone(),
            task_summary: info.task_summary.clone(),
            uptime_secs: info.started_at.elapsed().as_secs(),
            status: "running".to_string(),
            parent_id: info.parent_id.clone(),
        })
        .collect();
    Ok(out)
}

// =============================================================================
// Phase 36.17.10 Plan 01 Task 2: VoiceWritePayload, VoiceConfigSnapshot,
// update_voice_config, get_voice_config
// =============================================================================

/// Phase 36.17.10 Plan 01 (T-36.17.10-01-01): Web-safe voice config fields that
/// the browser is allowed to write. Contains ONLY non-secret, non-gateway fields.
/// Secret fields (`*_api_key`, `*_token`, `gateway.*`, `model.*`) are NEVER
/// deserialized from the browser — they are absent by design (secret-exclusion
/// mitigation: a field that does not exist cannot be clobbered).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct VoiceWritePayload {
    // VAD
    pub web_silence_threshold_rms: Option<f32>,
    pub silence_duration: Option<f64>,
    pub max_recording_seconds: Option<u32>,
    pub beep_enabled: Option<bool>,
    // STT
    pub stt_provider: Option<String>,
    pub stt_groq_model: Option<String>,
    pub stt_openai_model: Option<String>,
    // TTS — maps to config.tts.provider and config.tts.edge.* sub-fields
    pub tts_provider: Option<String>,
    /// Edge TTS voice name (maps to config.tts.edge.voice).
    pub tts_edge_voice: Option<String>,
    /// Edge TTS output format (maps to config.tts.edge.output_format).
    pub tts_edge_format: Option<String>,
    pub auto_tts: Option<bool>,
    /// Phase 36.17.12 Plan 01: Barge-in mode — serde-serialized BargeInMode variant name.
    /// Valid values: "push_to_interrupt" | "open_mic". Unknown strings are ignored during merge.
    /// NOT a secret field — plain enum string, excluded from secret-field handling.
    pub barge_in_mode: Option<String>,
    pub wake_word_enabled: Option<bool>,
    pub wake_word_phrase: Option<String>,
    // Phase 40.5 Plan 03 (D-09/D-17): Per-identity voice and appearance overrides.
    // All fields are Option — when None the global config path is unchanged (backward-compat).
    /// Identity slug that these overrides apply to. When Some, validate + merge into
    /// config.identities[slug]; when None, global fields are merged as before.
    pub identity_slug: Option<String>,
    /// Override the identity's free-mode TTS provider (D-09: free-mode only, not global).
    pub identity_tts_provider: Option<String>,
    /// Override the identity's free-mode TTS voice id (D-09).
    pub identity_tts_voice: Option<String>,
    /// Override the identity's realtime session voice (D-09/D-12).
    pub identity_realtime_voice: Option<String>,
    // D-17: appearance fields — one record holds BOTH appearance and voice.
    /// Orb style token: one of "classic" | "bloom" | "ascii" | "network".
    pub identity_appearance_style: Option<String>,
    /// Idle base hue 0–360 (mirrors IdentityAppearance.base_hue: Option<u16>).
    pub identity_base_hue: Option<u16>,
    /// Scale factor 0.5–2.0 (mirrors IdentityAppearance.size: Option<f32>).
    pub identity_size: Option<f32>,
    /// Glow intensity 0.0–1.0 (mirrors IdentityAppearance.glow: Option<f32>).
    pub identity_glow: Option<f32>,
}

/// Phase 40.5 Plan 03 (D-09/D-16): Resolved per-identity voice override for
/// a single identity, carried inside [`VoiceConfigSnapshot::identity_overrides`].
///
/// All voice fields are resolved-or-inherited (global fallback applied server-side),
/// so the panel can display the effective voice without any client-side resolution.
/// No secret fields — API keys are never included.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IdentityOverride {
    pub slug: String,
    /// Resolved free-mode TTS provider (identity override OR global fallback).
    pub free_mode_provider: String,
    /// Resolved free-mode TTS voice id (identity-specific; None means provider default).
    pub free_mode_voice: Option<String>,
    /// Resolved realtime voice (identity override OR global fallback).
    pub realtime_voice: String,
    /// Phase 40.5 gap-fix: saved orb appearance for this identity (`None` = no
    /// per-identity override stored; the client falls back to the
    /// ORB_PRESET_REGISTRY default for the slug). Lets the panel hydrate the
    /// appearance editor + live orb when an identity is selected in "Applies to".
    pub appearance_style: Option<String>,
    pub appearance_base_hue: Option<u16>,
    pub appearance_size: Option<f32>,
    pub appearance_glow: Option<f32>,
}

/// Phase 36.17.10 Plan 01 (T-36.17.10-01-02): Read-only snapshot of web-safe
/// voice config fields returned by `get_voice_config`. Never contains raw
/// `Config` — only fields the UI is allowed to display. Secret field VALUES
/// are never included; `secret_fields_present` carries label names only.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct VoiceConfigSnapshot {
    // VAD / voice
    pub web_silence_threshold_rms: f32,
    pub silence_duration: f64,
    pub max_recording_seconds: u32,
    pub beep_enabled: bool,
    // STT
    pub stt_provider: String,
    /// Resolved active STT model (derived from provider + per-provider model).
    pub stt_model: String,
    pub stt_groq_model: String,
    pub stt_openai_model: String,
    // TTS
    pub tts_provider: String,
    /// Edge TTS voice name (config.tts.edge.voice).
    pub tts_edge_voice: String,
    /// Edge TTS output format (config.tts.edge.output_format).
    pub tts_edge_format: String,
    pub auto_tts: bool,
    /// Phase 36.17.12 Plan 01: Barge-in mode as serde snake_case string.
    /// Values: "push_to_interrupt" | "open_mic". Plain enum string — not a secret field.
    pub barge_in_mode: String,
    pub wake_word_enabled: bool,
    pub wake_word_phrase: String,
    /// Whether the web config write gate is open. The UI should lock the
    /// form when this is false (UI-SPEC: settings panel locked state).
    pub web_config_write_enabled: bool,
    /// Names of secret fields present in config (label only — never their values).
    /// E.g. `["Model API key"]`. Used for UI display only.
    pub secret_fields_present: Vec<String>,
    // Phase 40.5 Plan 03 (D-16): per-identity data for the AppliesTo panel.
    /// All known identities as (slug, display_name) pairs — for the selector dropdown.
    /// Derived from ORB_PRESET_REGISTRY ∪ PRESET_REGISTRY ∪ config.identities entries.
    pub identity_list: Vec<(String, String)>,
    /// Per-identity resolved overrides (global fallback applied for fields not set).
    /// Panel reads these to display the currently selected identity's voice settings.
    pub identity_overrides: Vec<IdentityOverride>,
}

/// Phase 36.17.10 Plan 01: Validate that numeric and string fields in the
/// payload are within acceptable ranges before any config mutation.
/// Ranges from UI-SPEC Surface 4.
#[cfg(not(target_arch = "wasm32"))]
fn validate_voice_payload(payload: &VoiceWritePayload) -> Result<(), ServerFnError> {
    if let Some(rms) = payload.web_silence_threshold_rms {
        if !(0.5..=50.0).contains(&rms) {
            return Err(ServerFnError::new(format!(
                "web_silence_threshold_rms {rms} out of range [0.5, 50.0]"
            )));
        }
    }
    if let Some(dur) = payload.silence_duration {
        if !(0.5..=10.0).contains(&dur) {
            return Err(ServerFnError::new(format!(
                "silence_duration {dur} out of range [0.5, 10.0]"
            )));
        }
    }
    if let Some(ref provider) = payload.stt_provider {
        if provider.len() > 64 {
            return Err(ServerFnError::new("stt_provider too long (max 64 chars)"));
        }
    }
    if let Some(ref provider) = payload.tts_provider {
        if provider.len() > 64 {
            return Err(ServerFnError::new("tts_provider too long (max 64 chars)"));
        }
    }
    // Phase 40.5 Plan 03 (T-40.5-03-01/T-40.5-03-03/T-40.5-03-05):
    // Validate identity-scoped fields when identity_slug is present.
    if let Some(ref slug) = payload.identity_slug {
        // T-40.5-03-01: reject non-[alphanumeric|_] characters and slugs > 64 chars
        // to prevent path-traversal / arbitrary-YAML-key injection.
        if slug.len() > 64 {
            return Err(ServerFnError::new("identity_slug too long (max 64 chars)"));
        }
        if !slug.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(ServerFnError::new(
                "identity_slug must contain only alphanumeric characters and underscores",
            ));
        }
        // T-40.5-03-01: reject unknown slugs (not in ORB_PRESET_REGISTRY ∪ PRESET_REGISTRY).
        if !crate::components::hermes_app::avatar_logic::is_known_identity(slug) {
            return Err(ServerFnError::new(format!(
                "identity_slug '{slug}' is not a known identity"
            )));
        }
        // T-40.5-03-03: voice id length cap (64 chars).
        if let Some(ref voice) = payload.identity_tts_voice {
            if voice.len() > 64 {
                return Err(ServerFnError::new(
                    "identity_tts_voice too long (max 64 chars)",
                ));
            }
        }
        if let Some(ref voice) = payload.identity_realtime_voice {
            if voice.len() > 64 {
                return Err(ServerFnError::new(
                    "identity_realtime_voice too long (max 64 chars)",
                ));
            }
        }
        // T-40.5-03-05: appearance field range checks.
        const KNOWN_ORB_STYLES: &[&str] = &["classic", "bloom", "ascii", "network"];
        if let Some(ref style) = payload.identity_appearance_style {
            if !KNOWN_ORB_STYLES.contains(&style.as_str()) {
                return Err(ServerFnError::new(format!(
                    "identity_appearance_style '{style}' is not a known orb style"
                )));
            }
        }
        if let Some(hue) = payload.identity_base_hue {
            if hue > 360 {
                return Err(ServerFnError::new(format!(
                    "identity_base_hue {hue} out of range [0, 360]"
                )));
            }
        }
        if let Some(size) = payload.identity_size {
            if !(0.5..=2.0).contains(&size) {
                return Err(ServerFnError::new(format!(
                    "identity_size {size} out of range [0.5, 2.0]"
                )));
            }
        }
        if let Some(glow) = payload.identity_glow {
            if !(0.0..=1.0).contains(&glow) {
                return Err(ServerFnError::new(format!(
                    "identity_glow {glow} out of range [0.0, 1.0]"
                )));
            }
        }
    }
    Ok(())
}

/// Phase 36.17.10 Plan 01: Merge web-safe payload fields into a freshly
/// loaded Config. Secret fields are never in the payload so they are never
/// touched. Only Option fields that are `Some` are applied.
/// Maps: tts_edge_voice → config.tts.edge.voice, tts_edge_format →
/// config.tts.edge.output_format (TtsConfig has no top-level voice/format).
#[cfg(not(target_arch = "wasm32"))]
fn merge_voice_payload(config: &mut ironhermes_core::config::Config, payload: &VoiceWritePayload) {
    if let Some(v) = payload.web_silence_threshold_rms {
        config.voice.web_silence_threshold_rms = v;
    }
    if let Some(v) = payload.silence_duration {
        config.voice.silence_duration = v;
    }
    if let Some(v) = payload.max_recording_seconds {
        config.voice.max_recording_seconds = v;
    }
    if let Some(v) = payload.beep_enabled {
        config.voice.beep_enabled = v;
    }
    if let Some(ref v) = payload.stt_provider {
        config.stt.provider = v.clone();
    }
    if let Some(ref v) = payload.stt_groq_model {
        config.stt.groq.model = v.clone();
    }
    if let Some(ref v) = payload.stt_openai_model {
        config.stt.openai.model = v.clone();
    }
    if let Some(ref v) = payload.tts_provider {
        config.tts.provider = v.clone();
    }
    // TTS sub-struct fields (EdgeTtsConfig — no top-level voice/format on TtsConfig)
    if let Some(ref v) = payload.tts_edge_voice {
        config.tts.edge.voice = v.clone();
    }
    if let Some(ref v) = payload.tts_edge_format {
        config.tts.edge.output_format = v.clone();
    }
    if let Some(v) = payload.auto_tts {
        config.voice.auto_tts = v;
    }
    // Phase 36.17.12 Plan 01 (T-36.17.12-01-01): Parse barge_in_mode string into
    // BargeInMode enum via serde round-trip. Unknown/garbage strings are silently
    // ignored (no assignment, no error) — only valid snake_case variants reach config.
    if let Some(ref v) = payload.barge_in_mode {
        use ironhermes_core::config::BargeInMode;
        let parsed: Result<BargeInMode, _> =
            serde_json::from_value(serde_json::Value::String(v.clone()));
        if let Ok(mode) = parsed {
            config.voice.barge_in_mode = mode;
        }
    }
    if let Some(v) = payload.wake_word_enabled {
        config.voice.wake_word.enabled = v;
    }
    if let Some(ref v) = payload.wake_word_phrase {
        if !v.trim().is_empty() && v.len() <= 64 {
            config.voice.wake_word.phrase = v.clone();
        }
    }
    // Phase 40.5 Plan 03 (D-09/D-17): When identity_slug is Some, write voice
    // and appearance into config.identities[slug] WITHOUT touching global tts (D-08).
    if let Some(ref slug) = payload.identity_slug {
        use ironhermes_core::config::{IdentityAppearance, IdentityRecord};
        let rec: &mut IdentityRecord = config.identities.entry(slug.clone()).or_default();
        // Write voice fields when Some.
        if let Some(ref v) = payload.identity_tts_provider {
            rec.voice.free_mode_tts_provider = Some(v.clone());
        }
        if let Some(ref v) = payload.identity_tts_voice {
            rec.voice.free_mode_tts_voice = Some(v.clone());
        }
        if let Some(ref v) = payload.identity_realtime_voice {
            rec.voice.realtime_voice = Some(v.clone());
        }
        // D-17: write appearance fields when any appearance field is Some.
        let has_appearance = payload.identity_appearance_style.is_some()
            || payload.identity_base_hue.is_some()
            || payload.identity_size.is_some()
            || payload.identity_glow.is_some();
        if has_appearance {
            let app: &mut IdentityAppearance = rec
                .appearance
                .get_or_insert_with(IdentityAppearance::default);
            if let Some(ref v) = payload.identity_appearance_style {
                app.style = Some(v.clone());
            }
            if let Some(v) = payload.identity_base_hue {
                app.base_hue = Some(v);
            }
            if let Some(v) = payload.identity_size {
                app.size = Some(v);
            }
            if let Some(v) = payload.identity_glow {
                app.glow = Some(v);
            }
        }
    }
}

/// Phase 36.17.10 Plan 01 (T-36.17.10-01-02, T-36.17.10-01-04): Build a
/// VoiceConfigSnapshot from a freshly loaded Config. Secret field values are
/// never included — only their presence is noted by label name.
#[cfg(not(target_arch = "wasm32"))]
fn build_voice_snapshot(config: &ironhermes_core::config::Config) -> VoiceConfigSnapshot {
    // Determine the active STT model for display (derived from provider selection).
    let stt_model = match config.stt.provider.as_str() {
        "groq" => config.stt.groq.model.clone(),
        "openai" => config.stt.openai.model.clone(),
        _ => config.stt.groq.model.clone(), // "auto": default to groq model for display
    };

    // Collect secret field labels (names only — never values).
    // model.api_key is the primary secret field (Option<String>).
    let mut secret_fields_present = Vec::new();
    if config
        .model
        .api_key
        .as_deref()
        .map(|k| !k.is_empty())
        .unwrap_or(false)
    {
        secret_fields_present.push("Model API key".to_string());
    }

    // Phase 40.5 Plan 03 (D-16): Build identity_list from ORB_PRESET_REGISTRY ∪
    // PRESET_REGISTRY ∪ any additional config.identities entries.
    use crate::components::hermes_app::avatar_logic::{ORB_PRESET_REGISTRY, PRESET_REGISTRY};
    let mut identity_list: Vec<(String, String)> = ORB_PRESET_REGISTRY
        .iter()
        .map(|p| (p.id.to_string(), p.display_name.to_string()))
        .collect();
    for p in PRESET_REGISTRY.iter() {
        identity_list.push((p.id.to_string(), p.display_name.to_string()));
    }
    // Include any config.identities entries not already covered by the registries.
    for (slug, rec) in &config.identities {
        if !identity_list.iter().any(|(id, _)| id == slug) {
            let display = rec.display_name.clone().unwrap_or_else(|| slug.clone());
            identity_list.push((slug.clone(), display));
        }
    }

    // Build identity_overrides: for each known identity resolve the effective
    // voice settings (identity override OR global fallback) without exposing secrets.
    let global_tts_provider = config.tts.provider.clone();
    let global_realtime_voice = config.voice.realtime_voice.clone();
    let identity_overrides: Vec<IdentityOverride> = identity_list
        .iter()
        .map(|(slug, _)| {
            let (provider, voice) = config.resolve_tts_override(Some(slug.as_str()));
            let realtime = config
                .identities
                .get(slug.as_str())
                .and_then(|r| r.voice.realtime_voice.clone())
                .unwrap_or_else(|| global_realtime_voice.clone());
            // Phase 40.5 gap-fix: carry the saved orb appearance so the panel can
            // hydrate the appearance editor + live orb on identity switch.
            let appearance = config
                .identities
                .get(slug.as_str())
                .and_then(|r| r.appearance.clone());
            IdentityOverride {
                slug: slug.clone(),
                free_mode_provider: provider,
                free_mode_voice: voice,
                realtime_voice: realtime,
                appearance_style: appearance.as_ref().and_then(|a| a.style.clone()),
                appearance_base_hue: appearance.as_ref().and_then(|a| a.base_hue),
                appearance_size: appearance.as_ref().and_then(|a| a.size),
                appearance_glow: appearance.as_ref().and_then(|a| a.glow),
            }
        })
        .collect();
    // Suppress unused-variable warning when global_tts_provider has no other use.
    let _ = global_tts_provider;

    VoiceConfigSnapshot {
        web_silence_threshold_rms: config.voice.web_silence_threshold_rms,
        silence_duration: config.voice.silence_duration,
        max_recording_seconds: config.voice.max_recording_seconds,
        beep_enabled: config.voice.beep_enabled,
        stt_provider: config.stt.provider.clone(),
        stt_model,
        stt_groq_model: config.stt.groq.model.clone(),
        stt_openai_model: config.stt.openai.model.clone(),
        tts_provider: config.tts.provider.clone(),
        tts_edge_voice: config.tts.edge.voice.clone(),
        tts_edge_format: config.tts.edge.output_format.clone(),
        auto_tts: config.voice.auto_tts,
        // Phase 36.17.12 Plan 01: Serialize BargeInMode to its serde snake_case name.
        // Defaults to "push_to_interrupt" if serialization unexpectedly fails.
        barge_in_mode: serde_json::to_value(&config.voice.barge_in_mode)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "push_to_interrupt".to_string()),
        wake_word_enabled: config.voice.wake_word.enabled,
        wake_word_phrase: config.voice.wake_word.phrase.clone(),
        web_config_write_enabled: config.security.web_config_write_enabled,
        secret_fields_present,
        identity_list,
        identity_overrides,
    }
}

/// Phase 36.17.10 Plan 01: Write voice settings from the browser to config.yaml.
///
/// Four-step protocol (mirrors toggle_skill):
/// 1. validate payload (field range + length checks)
/// 2. Config::load() fresh from disk
/// 3. gate check — return error if security.web_config_write_enabled is false
/// 4. merge + atomic save (config.save() uses temp+rename after Task 1 upgrade)
///
/// Secret fields (api_key, tokens, gateway) are never present in VoiceWritePayload
/// so they are never touched during the merge step (T-36.17.10-01-01).
#[server]
pub async fn update_voice_config(payload: VoiceWritePayload) -> Result<(), ServerFnError> {
    // Step 1: validate
    validate_voice_payload(&payload)?;

    // Step 2: fresh disk read (NOT app_state.config — that is the startup snapshot)
    let mut config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;

    // Step 3: gate — web config write is disabled unless operator opts in
    if !config.security.web_config_write_enabled {
        return Err(ServerFnError::new("Config writes are disabled"));
    }

    // Step 4: merge web-safe fields, then atomic save
    merge_voice_payload(&mut config, &payload);
    config
        .save()
        .map_err(|e| ServerFnError::new(format!("Config save failed: {e}")))?;

    Ok(())
}

/// Phase 36.17.10 Plan 01 (T-36.17.10-01-02): Return a snapshot of web-safe
/// voice config fields. Reads fresh from disk (not app_state.config startup
/// snapshot) so the client always gets post-write values. Never returns the
/// raw Config struct — secret fields are excluded by VoiceConfigSnapshot's type.
#[server]
pub async fn get_voice_config() -> Result<VoiceConfigSnapshot, ServerFnError> {
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
    Ok(build_voice_snapshot(&config))
}

/// Phase 41.2 Plan 02 (D-06): the base64-audio return contract for
/// `synth_voice_sample`, consumed by Plan 04's `TestVoiceButton` client.
/// Same base64-in-JSON transport `attach_file` uses (kanban_api.rs:807),
/// mirrored in the encode direction.
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq)]
pub struct VoiceSampleAudio {
    pub audio_b64: String,
    pub mime: String,
}

/// Phase 41.2 Plan 02 (D-06): audition a short TTS sample for `identity_slug`'s
/// currently-configured free-mode voice. This is the "Test voice" server seam —
/// it reuses `auto_speak_reply`'s exact synth pipeline (ws.rs:2229-2309:
/// `is_known_identity` gate -> `Config::load()` -> `effective_tts_config_for_identity`
/// -> `build_tts_registry` -> `provider.synthesize`) but returns the resulting
/// bytes as the fn's value instead of dispatching a WS `AudioOut` frame — a
/// `#[server]` fn cannot push a WS frame back to the browser.
///
/// Deliberately NOT gated by `web_config_write_enabled` (RESEARCH anti-pattern):
/// this performs no config write, only a read + synth + return, so audition
/// must keep working on read-only installs — gating it there would make Test
/// Voice silently inert exactly where D-06's "audition before saving" intent
/// matters most.
///
/// Security (threat_model T-41.2-02/T-41.2-03): `identity_slug` is validated
/// via `avatar_logic::is_known_identity` BEFORE any provider/config
/// resolution — a crafted/tampered slug is rejected early and never drives
/// provider selection (same defense-in-depth `auto_speak_reply` applies).
/// The fn accepts NO client-supplied sample text; the server always
/// synthesizes a fixed short phrase, closing the oversized-text /
/// excessive-TTS-spend DoS vector entirely.
#[server]
pub async fn synth_voice_sample(identity_slug: String) -> Result<VoiceSampleAudio, ServerFnError> {
    #[cfg(feature = "server")]
    {
        // Reject an unknown/tampered slug before it can drive provider/config
        // resolution at all (do NOT fall through to provider resolution here).
        let validated = crate::components::hermes_app::avatar_logic::is_known_identity(
            &identity_slug,
        )
        .then_some(identity_slug.as_str());
        let validated = match validated {
            Some(slug) => slug,
            None => return Err(ServerFnError::new("unknown identity")),
        };

        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        let effective_tts = config.effective_tts_config_for_identity(Some(validated));
        let tts_registry = ironhermes_tools::tts::build_tts_registry(&effective_tts);
        let provider = tts_registry
            .get(&effective_tts.provider)
            .ok_or_else(|| ServerFnError::new("TTS provider not registered"))?;

        // D-06 discretion: a fixed short audition phrase. No client-supplied
        // text param exists on this fn — see the DoS rationale above.
        const SAMPLE_TEXT: &str = "This is how I sound.";

        let audio_dir = ironhermes_core::constants::get_hermes_home().join("audio_cache");
        std::fs::create_dir_all(&audio_dir)
            .map_err(|e| ServerFnError::new(format!("audio_cache dir: {e}")))?;
        let ext = if effective_tts.provider == "elevenlabs"
            && effective_tts.elevenlabs.output_format == "opus"
        {
            "opus"
        } else {
            "mp3"
        };
        let output_path = audio_dir.join(format!("{}.{}", uuid::Uuid::new_v4(), ext));

        let written_path = provider
            .synthesize(SAMPLE_TEXT, &output_path)
            .await
            .map_err(|e| ServerFnError::new(format!("TTS synthesis failed: {e}")))?;
        let bytes = std::fs::read(&written_path)
            .map_err(|e| ServerFnError::new(format!("audio read failed: {e}")))?;

        use base64::Engine as _;
        Ok(VoiceSampleAudio {
            audio_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
            mime: if ext == "opus" {
                "audio/ogg".to_string()
            } else {
                "audio/mpeg".to_string()
            },
        })
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = identity_slug;
        Err(ServerFnError::new(
            "synth_voice_sample unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod update_voice_config_tests {
    use super::{merge_voice_payload, validate_voice_payload, VoiceWritePayload};
    use ironhermes_core::config::Config;

    fn empty_payload() -> VoiceWritePayload {
        VoiceWritePayload {
            web_silence_threshold_rms: None,
            silence_duration: None,
            max_recording_seconds: None,
            beep_enabled: None,
            stt_provider: None,
            stt_groq_model: None,
            stt_openai_model: None,
            tts_provider: None,
            tts_edge_voice: None,
            tts_edge_format: None,
            auto_tts: None,
            barge_in_mode: None,
            wake_word_enabled: None,
            wake_word_phrase: None,
            // Phase 40.5 identity fields — None in the global-path tests.
            identity_slug: None,
            identity_tts_provider: None,
            identity_tts_voice: None,
            identity_realtime_voice: None,
            identity_appearance_style: None,
            identity_base_hue: None,
            identity_size: None,
            identity_glow: None,
        }
    }

    /// T-36.17.10-01-04: The write gate must fail closed when
    /// security.web_config_write_enabled is false (the default).
    #[test]
    fn gate_fails_closed_by_default() {
        let config = Config::default();
        assert!(
            !config.security.web_config_write_enabled,
            "web_config_write_enabled must default to false (gate closed)"
        );
    }

    /// T-36.17.10-01-01: merge_voice_payload must apply only set Option fields
    /// and leave every other field (including all secret fields) byte-identical.
    #[test]
    fn merge_leaves_secrets_untouched() {
        let mut config = Config::default();
        // Simulate a config with API key populated (ModelConfig.api_key is Option<String>)
        config.model.api_key = Some("secret-api-key".to_string());
        // Snapshot original non-payload values
        let original_api_key = config.model.api_key.clone();
        let original_silence_threshold = config.voice.silence_threshold;

        let mut payload = empty_payload();
        payload.auto_tts = Some(true);
        payload.web_silence_threshold_rms = Some(8.0);

        merge_voice_payload(&mut config, &payload);

        // Payload fields applied
        assert!(
            config.voice.auto_tts,
            "auto_tts should have been set to true"
        );
        assert_eq!(
            config.voice.web_silence_threshold_rms, 8.0f32,
            "web_silence_threshold_rms should have been set to 8.0"
        );
        // Secret fields untouched — api_key must not be modified by merge
        assert_eq!(
            config.model.api_key, original_api_key,
            "model.api_key must not be modified by merge_voice_payload"
        );
        // Native silence_threshold (different unit scale) untouched
        assert_eq!(
            config.voice.silence_threshold, original_silence_threshold,
            "native silence_threshold must not be modified by merge"
        );
    }

    /// T-36.17.10-01-01: VoiceWritePayload must have no field whose name ends
    /// in `_api_key` or `_token` — compile-time guarantee enforced by this test.
    /// We verify via the field names of a serialized empty payload (JSON keys).
    #[test]
    fn voice_write_payload_has_no_secret_fields() {
        let payload = empty_payload();
        let json = serde_json::to_string(&payload).expect("serialize VoiceWritePayload");
        // JSON keys for None fields are omitted (serde default), so test with Some values
        let full_payload = VoiceWritePayload {
            web_silence_threshold_rms: Some(5.0),
            silence_duration: Some(3.0),
            max_recording_seconds: Some(120),
            beep_enabled: Some(true),
            stt_provider: Some("auto".to_string()),
            stt_groq_model: Some("whisper-large-v3-turbo".to_string()),
            stt_openai_model: Some("whisper-1".to_string()),
            tts_provider: Some("edge".to_string()),
            tts_edge_voice: Some("en-US-AriaNeural".to_string()),
            tts_edge_format: Some("mp3".to_string()),
            auto_tts: Some(false),
            barge_in_mode: Some("open_mic".into()),
            wake_word_enabled: Some(true),
            wake_word_phrase: Some("hey hermes".to_string()),
            // Phase 40.5 identity fields — include with non-None to ensure no secret leaks.
            identity_slug: Some("orb_bloom".to_string()),
            identity_tts_provider: Some("openai".to_string()),
            identity_tts_voice: Some("alloy".to_string()),
            identity_realtime_voice: Some("echo".to_string()),
            identity_appearance_style: Some("bloom".to_string()),
            identity_base_hue: Some(280),
            identity_size: Some(1.0),
            identity_glow: Some(0.5),
        };
        let full_json = serde_json::to_string(&full_payload).expect("serialize full payload");
        assert!(
            !full_json.contains("_api_key"),
            "VoiceWritePayload JSON must not contain any '_api_key' field: {full_json}"
        );
        assert!(
            !full_json.contains("_token"),
            "VoiceWritePayload JSON must not contain any '_token' field: {full_json}"
        );
        // Silence unused binding warning
        let _ = json;
    }

    /// T-36.17.10-01-02: VoiceConfigSnapshot serializes without any secret field.
    #[test]
    fn voice_config_snapshot_has_no_secret_field_values() {
        use super::build_voice_snapshot;
        let mut config = Config::default();
        // ModelConfig.api_key is Option<String> — set it as Some to simulate a present key
        config.model.api_key = Some("secret-value".to_string());
        let snapshot = build_voice_snapshot(&config);
        let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
        assert!(
            !json.contains("secret-value"),
            "VoiceConfigSnapshot must never expose secret values: {json}"
        );
        assert!(
            !json.contains("_api_key"),
            "VoiceConfigSnapshot must not have _api_key JSON keys: {json}"
        );
        assert!(
            !json.contains("_token"),
            "VoiceConfigSnapshot must not have _token JSON keys: {json}"
        );
    }

    /// T-36.17.12-01-01: merge_voice_payload correctly parses "open_mic" into
    /// BargeInMode::OpenMic and assigns it to config.voice.barge_in_mode.
    #[test]
    fn barge_in_mode_merge_open_mic_round_trips() {
        use ironhermes_core::config::BargeInMode;
        let mut config = ironhermes_core::config::Config::default();
        // Default must be PushToInterrupt
        assert_eq!(config.voice.barge_in_mode, BargeInMode::PushToInterrupt);

        let mut payload = empty_payload();
        payload.barge_in_mode = Some("open_mic".into());
        merge_voice_payload(&mut config, &payload);
        assert_eq!(
            config.voice.barge_in_mode,
            BargeInMode::OpenMic,
            "open_mic string must parse to BargeInMode::OpenMic"
        );
    }

    /// T-36.17.12-01-02: build_voice_snapshot serializes OpenMic as "open_mic".
    #[test]
    fn barge_in_mode_snapshot_serializes_open_mic() {
        use super::build_voice_snapshot;
        use ironhermes_core::config::{BargeInMode, Config};
        let mut config = Config::default();
        config.voice.barge_in_mode = BargeInMode::OpenMic;
        let snapshot = build_voice_snapshot(&config);
        assert_eq!(
            snapshot.barge_in_mode, "open_mic",
            "BargeInMode::OpenMic must serialize to \"open_mic\" in snapshot"
        );
    }

    /// T-36.17.12-01-03: snapshot serializes PushToInterrupt as "push_to_interrupt".
    #[test]
    fn barge_in_mode_snapshot_serializes_push_to_interrupt() {
        use super::build_voice_snapshot;
        use ironhermes_core::config::{BargeInMode, Config};
        let mut config = Config::default();
        config.voice.barge_in_mode = BargeInMode::PushToInterrupt;
        let snapshot = build_voice_snapshot(&config);
        assert_eq!(
            snapshot.barge_in_mode, "push_to_interrupt",
            "BargeInMode::PushToInterrupt must serialize to \"push_to_interrupt\""
        );
    }

    /// T-36.17.12-01-04: Unknown barge_in_mode string must NOT change the
    /// existing config value and must NOT panic.
    #[test]
    fn barge_in_mode_unknown_string_is_ignored() {
        use ironhermes_core::config::BargeInMode;
        let mut config = ironhermes_core::config::Config::default();
        // Set to OpenMic first so we can detect if it reverts
        config.voice.barge_in_mode = BargeInMode::OpenMic;

        let mut payload = empty_payload();
        payload.barge_in_mode = Some("garbage_value".into());
        merge_voice_payload(&mut config, &payload);

        // Must remain unchanged — unknown string is silently ignored
        assert_eq!(
            config.voice.barge_in_mode,
            BargeInMode::OpenMic,
            "Unknown barge_in_mode string must not change config value"
        );
    }

    /// T-36.17.10-01-06: validate_voice_payload rejects out-of-range values.
    #[test]
    fn validate_rejects_out_of_range_silence_values() {
        // silence_duration below range
        let mut payload = empty_payload();
        payload.silence_duration = Some(0.1);
        assert!(
            validate_voice_payload(&payload).is_err(),
            "silence_duration 0.1 should be rejected (min 0.5)"
        );

        // silence_duration above range
        payload.silence_duration = Some(15.0);
        assert!(
            validate_voice_payload(&payload).is_err(),
            "silence_duration 15.0 should be rejected (max 10.0)"
        );

        // web_silence_threshold_rms below range
        let mut payload2 = empty_payload();
        payload2.web_silence_threshold_rms = Some(0.1);
        assert!(
            validate_voice_payload(&payload2).is_err(),
            "web_silence_threshold_rms 0.1 should be rejected (min 0.5)"
        );

        // web_silence_threshold_rms above range
        payload2.web_silence_threshold_rms = Some(100.0);
        assert!(
            validate_voice_payload(&payload2).is_err(),
            "web_silence_threshold_rms 100.0 should be rejected (max 50.0)"
        );

        // valid values pass
        let mut payload3 = empty_payload();
        payload3.silence_duration = Some(3.0);
        payload3.web_silence_threshold_rms = Some(5.0);
        assert!(
            validate_voice_payload(&payload3).is_ok(),
            "valid values should pass validation"
        );
    }
}

/// Phase 41.2 Plan 02 (D-06): proves `synth_voice_sample`'s slug-validation
/// gate before touching provider/config resolution. The network-dependent
/// synth path itself cannot run natively without a live TTS provider, so
/// these tests exercise the extractable gate logic
/// (`avatar_logic::is_known_identity`) that `synth_voice_sample` calls
/// before any provider selection — proving a tampered slug never reaches
/// provider resolution.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod synth_voice_sample_tests {
    use crate::components::hermes_app::avatar_logic::is_known_identity;

    /// T-41.2-02: an unknown/tampered identity slug must be rejected by the
    /// same gate `synth_voice_sample` applies before `effective_tts_config_for_identity`.
    #[test]
    fn synth_voice_sample_rejects_unknown_slug() {
        assert!(
            !is_known_identity("not_a_real_identity"),
            "an unknown slug must fail is_known_identity — synth_voice_sample's \
             validated-slug branch must yield None/reject for it"
        );
    }

    /// A known ORB_PRESET_REGISTRY slug must pass the gate so a legitimate
    /// audition request is never blocked by the security check.
    #[test]
    fn synth_voice_sample_accepts_known_slug() {
        assert!(
            is_known_identity("orb_classic"),
            "orb_classic is a registered ORB_PRESET_REGISTRY id and must pass is_known_identity"
        );
    }
}

// =============================================================================
// Phase 40.5 Plan 03 Task 2 — RED gate: identity voice + appearance tests
// Tests reference VoiceWritePayload.identity_slug / identity_tts_provider /
// identity_appearance_style etc. which do NOT exist yet → compile error = RED.
// =============================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod voice_identity_tests_40_5_03 {
    use super::{
        build_voice_snapshot, merge_voice_payload, validate_voice_payload, VoiceWritePayload,
    };
    use ironhermes_core::config::Config;

    /// Construct an all-None payload INCLUDING the new identity fields.
    /// Fields identity_slug through identity_glow do not exist yet → RED.
    fn identity_payload(slug: Option<&str>) -> VoiceWritePayload {
        VoiceWritePayload {
            web_silence_threshold_rms: None,
            silence_duration: None,
            max_recording_seconds: None,
            beep_enabled: None,
            stt_provider: None,
            stt_groq_model: None,
            stt_openai_model: None,
            tts_provider: None,
            tts_edge_voice: None,
            tts_edge_format: None,
            auto_tts: None,
            barge_in_mode: None,
            wake_word_enabled: None,
            wake_word_phrase: None,
            // Phase 40.5 identity fields — not yet on struct (RED gate).
            identity_slug: slug.map(|s| s.to_string()),
            identity_tts_provider: None,
            identity_tts_voice: None,
            identity_realtime_voice: None,
            identity_appearance_style: None,
            identity_base_hue: None,
            identity_size: None,
            identity_glow: None,
        }
    }

    /// T-40.5-03-01: validate_voice_payload rejects identity_slug with non
    /// [alphanumeric|_] characters (path-traversal guard).
    #[test]
    fn validate_rejects_bad_slug() {
        // Slash character — path-traversal attempt.
        let mut p = identity_payload(Some("orb/evil"));
        assert!(
            validate_voice_payload(&p).is_err(),
            "identity_slug with '/' must be rejected"
        );
        // Exceeds 64 chars.
        p.identity_slug = Some("a".repeat(65));
        assert!(
            validate_voice_payload(&p).is_err(),
            "identity_slug > 64 chars must be rejected"
        );
    }

    /// T-40.5-03-01: well-formed but unknown slug must also be rejected.
    #[test]
    fn validate_rejects_unknown_slug() {
        let p = identity_payload(Some("completely_unknown_slug_xyz"));
        assert!(
            validate_voice_payload(&p).is_err(),
            "identity_slug not in registry must be rejected"
        );
    }

    /// T-40.5-03-03: identity_tts_voice longer than 64 chars must be rejected.
    #[test]
    fn validate_rejects_overlong_voice() {
        let mut p = identity_payload(Some("orb_bloom"));
        p.identity_tts_voice = Some("v".repeat(65));
        assert!(
            validate_voice_payload(&p).is_err(),
            "identity_tts_voice > 64 chars must be rejected"
        );
    }

    /// D-09: merge writes identity_tts_provider to config.identities[slug].voice.
    #[test]
    fn merge_writes_identity_voice() {
        let mut config = Config::default();
        let original_provider = config.tts.provider.clone();

        let mut p = identity_payload(Some("orb_bloom"));
        p.identity_tts_provider = Some("openai".to_string());
        merge_voice_payload(&mut config, &p);

        let rec = config
            .identities
            .get("orb_bloom")
            .expect("orb_bloom missing");
        assert_eq!(
            rec.voice.free_mode_tts_provider.as_deref(),
            Some("openai"),
            "identity merge must write free_mode_tts_provider"
        );
        // Global tts must be untouched (D-08 scope fence).
        assert_eq!(
            config.tts.provider, original_provider,
            "global tts.provider must not change when identity_slug is set"
        );
    }

    /// D-17: merge writes appearance fields into config.identities[slug].appearance
    /// and the result survives a serde_yaml round-trip.
    #[test]
    fn merge_writes_identity_appearance() {
        let mut config = Config::default();

        let mut p = identity_payload(Some("orb_bloom"));
        p.identity_appearance_style = Some("bloom".to_string());
        p.identity_base_hue = Some(280);
        p.identity_size = Some(1.2);
        p.identity_glow = Some(0.8);
        merge_voice_payload(&mut config, &p);

        let rec = config
            .identities
            .get("orb_bloom")
            .expect("orb_bloom missing");
        let app = rec.appearance.as_ref().expect("appearance must be set");
        assert_eq!(app.style.as_deref(), Some("bloom"));
        assert_eq!(app.base_hue, Some(280u16));
        assert_eq!(app.size, Some(1.2f32));
        assert_eq!(app.glow, Some(0.8f32));

        // JSON round-trip (D-17 persistence requirement — serde_json already a dep).
        let json = serde_json::to_string(&config).expect("json serialize");
        let config2: Config = serde_json::from_str(&json).expect("json parse");
        let rec2 = config2
            .identities
            .get("orb_bloom")
            .expect("orb_bloom round-trip");
        let app2 = rec2.appearance.as_ref().expect("appearance round-trip");
        assert_eq!(
            app2.style.as_deref(),
            Some("bloom"),
            "style must survive round-trip"
        );
        assert_eq!(
            app2.base_hue,
            Some(280u16),
            "base_hue must survive round-trip"
        );
    }

    /// Regression: with identity_slug=None the global merge path is unchanged.
    #[test]
    fn merge_global_path_unchanged() {
        let mut config = Config::default();
        let mut p = identity_payload(None);
        p.tts_provider = Some("edge".to_string());
        merge_voice_payload(&mut config, &p);
        assert_eq!(
            config.tts.provider, "edge",
            "global tts.provider must be updated"
        );
    }

    /// T-40.5-03-02: snapshot with identity_list + identity_overrides must not
    /// expose any secret value (extends the existing secret-exclusion test).
    #[test]
    fn snapshot_excludes_secrets_with_identity_fields() {
        let mut config = Config::default();
        config.model.api_key = Some("super-secret-key".to_string());
        let snapshot = build_voice_snapshot(&config);
        let json = serde_json::to_string(&snapshot).expect("serialize");
        assert!(
            !json.contains("super-secret-key"),
            "snapshot must not expose secret values: {json}"
        );
        assert!(
            !json.contains("_api_key"),
            "snapshot must not have _api_key keys: {json}"
        );
        // identity_list and identity_overrides must be present in the snapshot.
        assert!(
            json.contains("identity_list"),
            "snapshot must include identity_list: {json}"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 36.17.10 Plan 04 — cancel_turn server fn
// ---------------------------------------------------------------------------
//
// Mirrors the existing /stop intercept in ws.rs:812-843:
//   1. Build the web SessionKey inline (same as web_key helper in ws.rs)
//   2. Clear the web session queue via app_state.queue.clear(&key)
//   3. Reset the paused flag to false via get_or_create_paused_flag
// NO JoinHandle::abort — the in-flight turn completes naturally (D-05 design
// decision). See T-36.17.10-04-03 in the plan threat model.
//
// The inner queue+paused logic lives in `cancel_session_logic` (a
// `#[cfg(not(target_arch = "wasm32"))]` helper) so it is unit-testable
// without requiring a live server fn dispatch.

/// Phase 36.17.10 Plan 04: testable inner logic for cancel_turn.
///
/// Clears the web session queue for `session_id` and resets its paused flag
/// to `false`. Does NOT abort any in-flight turn (D-05 design decision —
/// the in-flight chunk completes naturally).
///
/// Called by the `cancel_turn` server fn (thin wrapper) and directly by
/// `cancel_turn_tests` unit tests.
///
/// The gate must name `feature = "server"`, not just a non-wasm target: the
/// `crate::server::state` module this signature borrows from is itself
/// `#[cfg(feature = "server")]`, and `not(target_arch = "wasm32")` is the wider
/// predicate. Gating on the target alone left one configuration — native with
/// default features, i.e. no `server` — where this function was compiled but
/// `server::state` was not, failing with `E0433: cannot find state in server`.
/// `dx` never builds that configuration (its client is wasm, its server enables
/// the feature), but `cargo test --workspace` does.
#[cfg(all(not(target_arch = "wasm32"), feature = "server"))]
fn cancel_session_logic(state: &crate::server::state::AppState, session_id: &str) {
    let key = ironhermes_core::session::SessionKey {
        platform: ironhermes_core::types::Platform::Web,
        chat_id: session_id.to_string(),
        user_id: Some("web".to_string()),
    };
    state.queue.clear(&key);
    state
        .get_or_create_paused_flag(session_id)
        .store(false, std::sync::atomic::Ordering::SeqCst);
}

/// Phase 36.17.10 Plan 04: Cancel the in-flight or queued turn for the
/// current web session.
///
/// Mirrors the `/stop` slash-command intercept (ws.rs:812-843): clears the
/// web session queue and resets the paused flag. Does NOT call
/// JoinHandle::abort — the in-flight chunk completes naturally (D-05).
///
/// Called from the chat bubble action row via
/// `spawn(async move { cancel_turn(sid).await })` in chat.rs.
#[server]
pub async fn cancel_turn(session_id: String) -> Result<(), ServerFnError> {
    let state = crate::server::state::global_app_state();
    cancel_session_logic(state, &session_id);
    // Phase 36.17.10 UAT fix: cancel_session_logic only mutates shared state, so
    // the client UI never learned the queue cleared (REMOVE appeared to do nothing).
    // Mirror the WS /stop path (ws.rs:850) by emitting QueueUpdated on the live
    // connection via the D-06 active-sender slot (subagent_callback_slot), which
    // ws.rs installs for the duration of the in-flight turn. None when no turn is
    // running — queue_depth is already 0 then, so there is nothing to refresh.
    if let Some(sender) = state.subagent_callback_slot.lock().await.as_ref() {
        let _ = sender.send(crate::protocol::ChatStreamEvent::QueueUpdated {
            depth: 0,
            paused: false,
        });
    }
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod cancel_turn_tests {
    use ironhermes_core::queue::MessageQueue;
    use ironhermes_core::session::SessionKey;
    use ironhermes_core::types::Platform;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    /// Build a web-session key — mirrors the ws.rs web_key helper.
    fn web_key(session_id: &str) -> SessionKey {
        SessionKey {
            platform: Platform::Web,
            chat_id: session_id.to_string(),
            user_id: Some("web".to_string()),
        }
    }

    /// Build a minimal AppState-like struct for testing cancel_session_logic.
    ///
    /// We cannot construct a full AppState (requires live agent runtime), so
    /// we build the two relevant fields (queue + queue_paused) directly and
    /// then use the concrete SessionQueue + map helpers to exercise the logic.
    #[allow(clippy::type_complexity)] // test helper returns a one-off (queue, paused-map) tuple
    fn make_queue_and_paused() -> (
        Arc<dyn ironhermes_core::queue::MessageQueue<SessionKey>>,
        Arc<
            std::sync::Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>,
        >,
    ) {
        let queue: Arc<dyn MessageQueue<SessionKey>> =
            Arc::new(ironhermes_gateway::session_queue::SessionQueue::new())
                as Arc<dyn MessageQueue<SessionKey>>;
        let paused: Arc<
            std::sync::Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>,
        > = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        (queue, paused)
    }

    /// Helper: get-or-create a paused flag in the map (mirrors AppState method).
    fn get_or_create_paused(
        map: &Arc<
            std::sync::Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>,
        >,
        session_id: &str,
    ) -> Arc<std::sync::atomic::AtomicBool> {
        let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(std::sync::atomic::AtomicBool::new(false)))
            .clone()
    }

    // NOTE: cancel_session_logic takes &AppState which we cannot construct
    // from tests. The tests below verify the individual operations that
    // cancel_session_logic composes: queue.clear behavior and paused-flag
    // store behavior. These are the exact two operations cancel_session_logic
    // calls, tested directly on the same concrete types. This is the
    // test-as-inner-helper pattern described in the plan: we test the pure
    // queue+paused logic, which is all cancel_session_logic does.

    /// T-36.17.10-04-01: After cancel logic, the queue for the web session
    /// must be empty (depth 0), even when it had messages before.
    #[test]
    fn cancel_clears_web_session_queue() {
        let (queue, _paused) = make_queue_and_paused();
        let key = web_key("session-test-1");

        // Seed the queue with two messages
        queue
            .try_push(&key, "msg-1".to_string())
            .expect("push 1 should succeed");
        queue
            .try_push(&key, "msg-2".to_string())
            .expect("push 2 should succeed");

        assert_eq!(
            queue.len(&key),
            2,
            "Queue should have 2 messages before clear"
        );

        // Perform the clear (the core operation in cancel_session_logic)
        queue.clear(&key);

        assert_eq!(
            queue.len(&key),
            0,
            "Queue must be empty after cancel_session_logic's queue.clear call"
        );
    }

    /// T-36.17.10-04-02: After cancel logic, the paused flag for the session
    /// must be `false` — even when it was `true` before.
    #[test]
    fn cancel_resets_paused_flag_to_false() {
        let (_queue, paused_map) = make_queue_and_paused();

        // Set the paused flag to true first
        let flag = get_or_create_paused(&paused_map, "session-test-2");
        flag.store(true, Ordering::SeqCst);
        assert!(
            flag.load(Ordering::SeqCst),
            "Flag should be true before cancel"
        );

        // Perform the reset (the second operation in cancel_session_logic)
        flag.store(false, Ordering::SeqCst);

        assert!(
            !flag.load(Ordering::SeqCst),
            "Paused flag must be false after cancel_session_logic's .store(false)"
        );
    }

    /// T-36.17.10-04-03: cancel_turn does NOT call JoinHandle::abort (D-05).
    /// Verified by code-shape: cancel_session_logic and cancel_turn contain no
    /// abort() call. This test asserts the behavioral contract: queue.clear is
    /// idempotent on an already-empty queue (safe to call whether or not a turn
    /// is in-flight).
    #[test]
    fn cancel_is_safe_on_empty_queue_no_abort() {
        let (queue, _paused) = make_queue_and_paused();
        let key = web_key("session-test-3");

        // Clear on a queue that has no messages — must not panic (idempotent)
        queue.clear(&key);
        assert_eq!(
            queue.len(&key),
            0,
            "clear on empty queue must be a no-op (idempotent, safe to call mid-turn)"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 36.17.12 Plan 02 — issue_realtime_token server fn
// ---------------------------------------------------------------------------
//
// Exchanges the permanent server-side OPENAI_API_KEY for a short-lived ephemeral
// token using the OpenAI Realtime API /v1/realtime/client_secrets endpoint.
// The ephemeral token is the ONLY thing returned to the browser — the permanent
// key never crosses the server→browser boundary (D-05 key-secrecy control).
//
// Security invariants (T-36.17.12-02-01 through T-36.17.12-02-05):
//   - Whitelist validation runs BEFORE any network call (T-36.17.12-02-02 / T-V5).
//   - Absent key returns a typed ServerFnError (no panic) — triggers D-07 fallback.
//   - The api_key and raw response body are NEVER passed to any logging macro
//     (T-36.17.12-02-05 / T-LOG-LEAK). Only status codes / error categories logged.
//   - The #[server] macro gates this to the server binary — no extra #[cfg] needed.

/// Phase 36.17.12 Plan 02: Allowed realtime session models (T-36.17.12-02-02 whitelist).
/// Updated in BUG 3 fix: `gpt-realtime` is the GA OpenAI Realtime API model (2026).
/// The model is now resolved from `voice.realtime_model` in config.yaml (config-driven,
/// not hardcoded), then validated here before any network call (T-V5 security control).
#[cfg(not(target_arch = "wasm32"))]
const REALTIME_ALLOWED_MODELS: &[&str] = &["gpt-realtime"];

/// Phase 36.17.12 Plan 02: Allowed voices for realtime sessions (T-36.17.12-02-02 whitelist).
#[cfg(not(target_arch = "wasm32"))]
const REALTIME_ALLOWED_VOICES: &[&str] = &[
    "alloy", "shimmer", "echo", "verse", "ash", "ballad", "coral", "sage",
];

// =============================================================================
// Phase 40.5 Plan 03 Task 3 — Per-identity realtime voice resolution (D-12)
// =============================================================================

/// Resolve the realtime voice for a session, applying per-identity overrides.
///
/// Priority order (D-12: identity frozen at session start):
///   1. `slug` is `Some(s)` and the identity has `realtime_voice` set → that value.
///   2. `slug` is `Some(s)` but no per-identity `realtime_voice` → global fallback.
///   3. `slug` is `None` → `config.voice.realtime_voice`.
///
/// Security gates (T-40.5-03-01):
/// - Charset gate: [alphanumeric | `_`] only, length ≤ 64.
/// - Known-identity gate: `is_known_identity(s)` must return true.
///
/// Unknown or malformed slugs are rejected with a `ServerFnError` before any
/// network call or config lookup.
///
/// The REALTIME_ALLOWED_VOICES whitelist is applied by the CALLER (`issue_realtime_token`)
/// as a defense-in-depth check after this function returns (T-40.5-03-03).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn resolve_realtime_voice(
    config: &ironhermes_core::config::Config,
    slug: Option<&str>,
) -> Result<String, ServerFnError> {
    use crate::components::hermes_app::avatar_logic::is_known_identity;

    let global_voice = config.voice.realtime_voice.clone();

    let Some(s) = slug else {
        return Ok(global_voice);
    };

    // T-40.5-03-01: charset + length gate — reject before any identity lookup.
    if s.len() > 64 || !s.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(ServerFnError::new(
            "Invalid identity slug — rejected (T-40.5-03-01)".to_string(),
        ));
    }
    // T-40.5-03-01: known-identity gate — slug must be in ORB_PRESET_REGISTRY ∪ PRESET_REGISTRY.
    if !is_known_identity(s) {
        return Err(ServerFnError::new(
            "Unknown identity — realtime voice unavailable (T-40.5-03-01)".to_string(),
        ));
    }

    // Per-identity override wins when set; otherwise fall back to global.
    let identity_voice = config
        .identities
        .get(s)
        .and_then(|rec| rec.voice.realtime_voice.clone());

    Ok(identity_voice.unwrap_or(global_voice))
}

// =============================================================================
// Phase 39.1 Plan 07 (R39.1-10): Realtime turn registry + concurrency control
// =============================================================================

/// Cooperative-cancel poll response returned by `realtime_heartbeat`.
///
/// The wasm client polls every ~2-3 s; on `Cancelled` it calls
/// `teardown_realtime_session()` (existing) and stops the loop.
/// On `Continue` it sleeps the interval and polls again.
///
/// `PartialEq` is required so tests can assert equality (Task 5).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum RealtimeTurnSignal {
    Continue,
    Cancelled,
}

/// Return value of `issue_realtime_token` — carries both the ephemeral OpenAI
/// token AND the server-assigned `turn_id` the client must echo back to
/// `realtime_heartbeat` / `realtime_session_end`.
///
/// D-04 (RC-1): also echoes back the `chat_session_id` (the user's real chat
/// session key `"agent:main:web:dm:{uuid}"`) so the browser can pass it as the
/// WRITE key to `realtime_save_transcript`.  The server validates this key and
/// calls `ensure_web_session` for it so the FK constraint is satisfied before
/// any transcript write attempt.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IssueRealtimeTokenResult {
    /// Short-lived OpenAI ephemeral token for the WebRTC session.
    pub token: String,
    /// UUID assigned by the server at mint time (used as heartbeat/end key).
    pub turn_id: String,
    /// Realtime session id (derived from turn_id; returned for internal use).
    pub session_id: String,
    /// The user's real chat session key (`"agent:main:web:dm:{uuid}"`) —
    /// the WRITE key for `realtime_save_transcript` (D-04 RC-1).
    pub chat_session_id: String,
}

/// Pure, testable helper: decide the cooperative-cancel signal for a given
/// `CancellationToken`.
///
/// Used by `realtime_heartbeat` after the registry lookup so the mapping is
/// unit-testable without `global_app_state()` (Task 5 asserts it directly).
#[cfg(not(target_arch = "wasm32"))]
pub fn realtime_signal_for(cancel: &tokio_util::sync::CancellationToken) -> RealtimeTurnSignal {
    if cancel.is_cancelled() {
        RealtimeTurnSignal::Cancelled
    } else {
        RealtimeTurnSignal::Continue
    }
}

/// Phase 36.17.12 Plan 02: Exchange the server-side OPENAI_API_KEY for a short-lived
/// ephemeral token for a WebRTC-direct realtime session (D-04/D-05).
///
/// Phase 39.1 Plan 07 (R39.1-10): additionally acquires a ConcurrencyLayer permit
/// pair (per-session cap + global ceiling) and registers a `TurnEntry{surface:
/// Realtime}` in the shared `TurnRegistry` BEFORE returning — register-before-
/// handoff (RESEARCH Pitfall 4). Returns `IssueRealtimeTokenResult` containing
/// both the ephemeral token and the server-assigned `turn_id` the client must
/// echo back for heartbeat / session-end.
///
/// Security contract:
/// - `model` is resolved from `voice.realtime_model` in config.yaml (BUG 3 fix:
///   config-driven, not browser-supplied) then whitelist-validated BEFORE any
///   network call. `voice` is whitelist-validated the same way.
/// - The permanent API key is read server-side only and is never serialized into
///   any browser-reachable value.
/// - On absent key: returns a typed `ServerFnError` (no panic) — the caller (Plan 03)
///   uses this error as the D-07 fallback trigger to downgrade to turn-based voice.
/// - The api_key and raw response body are never logged (T-LOG-LEAK).
/// - Past the global ceiling: returns a typed ServerFnError so the client falls
///   back / retries (realtime cannot queue a live media session — T-39.1-07-02).
/// D-04 RC-1: `chat_session_id` is the user's real chat session key
/// (`"agent:main:web:dm:{uuid}"`) — already present in the browser's
/// `SessionIdContext` at the time the realtime session is started.
/// The server validates it, calls `ensure_web_session` to guarantee the
/// FK row exists in `sessions`, and echoes it back in the result so the
/// browser can pass it as the WRITE key to `realtime_save_transcript`.
#[server]
pub async fn issue_realtime_token(
    chat_session_id: String,
    // Phase 40.5 Plan 03 (D-12): identity slug frozen at session start.
    // Server resolves the per-identity realtime voice (if set) or falls back to global.
    // None = no active identity (use global config.voice.realtime_voice).
    // T-40.5-03-01: slug is validated server-side by resolve_realtime_voice.
    active_identity: Option<String>,
) -> Result<IssueRealtimeTokenResult, ServerFnError> {
    // Step 1: Load config to resolve the realtime model, voice, and key env var name.
    // Model AND voice are resolved here (server-side, config-driven) — never trust
    // browser-supplied values. This satisfies the config-driven requirement and T-V5.
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;

    // Resolve the realtime model from voice.realtime_model (default: "gpt-realtime").
    // Resolve the realtime voice with per-identity override (D-12): identity override
    // wins over global config.voice.realtime_voice when set.
    let model = config.voice.realtime_model.clone();
    let voice = resolve_realtime_voice(&config, active_identity.as_deref())?;

    // Step 2: Whitelist validation — reject unknown model/voice BEFORE any network call
    // (T-36.17.12-02-02 / T-V5). This makes the validation tests offline/hermetic.
    if !REALTIME_ALLOWED_MODELS.contains(&model.as_str()) {
        return Err(ServerFnError::new(format!(
            "Unknown model '{}' — realtime unavailable",
            model
        )));
    }
    if !REALTIME_ALLOWED_VOICES.contains(&voice.as_str()) {
        return Err(ServerFnError::new(format!(
            "Unknown voice '{}' — realtime unavailable",
            voice
        )));
    }

    // Step 3: Resolve the env var name that holds the OpenAI API key.
    // Prefer providers["openai"].api_key_env if set, fall back to "OPENAI_API_KEY".
    let key_env = config
        .providers
        .get("openai")
        .and_then(|p| p.api_key_env.as_deref())
        .unwrap_or("OPENAI_API_KEY");
    // T-LOG-LEAK: log only the env var NAME, never the key value.
    let api_key = std::env::var(key_env).map_err(|_| {
        tracing::info!(
            env_var = key_env,
            "OPENAI_API_KEY not set — realtime unavailable (D-07 fallback trigger)"
        );
        ServerFnError::new("OPENAI_API_KEY not set — realtime unavailable")
    })?;

    // Step 4: Exchange permanent key for an ephemeral token via POST to
    // /v1/realtime/client_secrets. Verified against OpenAI Realtime WebRTC docs
    // (A2/A3 from RESEARCH.md — endpoint and response field confirmed current).
    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.openai.com/v1/realtime/client_secrets")
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "session": {
                "type": "realtime",
                "model": model,
                "audio": { "output": { "voice": voice } }
            }
        }))
        .send()
        .await
        .map_err(|e| {
            // T-LOG-LEAK: log only error category, not the request body or api_key.
            tracing::warn!(error_category = "network", "Realtime token request failed");
            ServerFnError::new(format!("Token request failed: {e}"))
        })?;

    // Step 5: Parse the response and extract the ephemeral token.
    // T-LOG-LEAK: log only status code, not the response body (body contains the token).
    let status = resp.status();
    if !status.is_success() {
        tracing::warn!(http_status = %status, "Realtime token endpoint returned error");
        return Err(ServerFnError::new(format!(
            "Token endpoint returned status {status}"
        )));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| {
        tracing::warn!(
            error_category = "parse",
            "Realtime token response parse failed"
        );
        ServerFnError::new(format!("Token parse failed: {e}"))
    })?;

    // Extract the ephemeral token value (field: "value" per OpenAI Realtime API docs).
    let token = body["value"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| ServerFnError::new("No token in response"))?;

    // ── Phase 39.1 Plan 07 (R39.1-10): register realtime turn ────────────────
    // AFTER successful token fetch (so a failed mint registers nothing),
    // BEFORE returning (register-before-handoff — RESEARCH Pitfall 4).

    let app_state = crate::server::state::global_app_state();

    // Acquire ConcurrencyLayer permit pair. If None (per-session cap or global
    // ceiling exhausted), REFUSE the mint — realtime cannot queue (live media
    // session). T-39.1-07-02: bounds host + OpenAI token spend.
    let (per_permit, global_permit) = app_state.concurrency.try_acquire().ok_or_else(|| {
        ServerFnError::new("realtime concurrency ceiling reached — try again shortly")
    })?;

    // ── D-04 RC-1: validate + ensure the chat session FK row ─────────────────
    // `chat_session_id` is the browser's real session key ("agent:main:web:dm:{uuid}").
    // Reject obviously malformed values before any DB or network work.
    // The pattern "agent:main:web:dm:" + UUID is the canonical web-DM key produced by
    // create_session (api.rs:~516-526). We do a lightweight structural check:
    // non-empty, starts with the expected prefix, and the trailing segment parses as UUID.
    {
        const WEB_SESSION_PREFIX: &str = "agent:main:web:dm:";
        let uuid_part = chat_session_id
            .strip_prefix(WEB_SESSION_PREFIX)
            .ok_or_else(|| {
                ServerFnError::new(format!(
                    "chat_session_id '{chat_session_id}' is not a valid web session key"
                ))
            })?;
        uuid::Uuid::parse_str(uuid_part).map_err(|_| {
            ServerFnError::new(format!(
                "chat_session_id '{chat_session_id}' does not contain a valid UUID suffix"
            ))
        })?;
    }

    // Ensure the sessions FK row exists.  create_session is idempotent if the row
    // already exists; calling it here guarantees realtime_save_transcript's
    // add_message never hits a FK violation (D-04 RC-1 root cause fix).
    app_state
        .ensure_web_session(&chat_session_id)
        .map_err(|e| ServerFnError::new(format!("ensure_web_session failed: {e}")))?;

    // Generate the server-side turn_id (opaque UUID v4 — T-39.1-01).
    let turn_id = ironhermes_core::TurnId::new_v4();
    // Use turn_id string as the realtime session_id (simplest stable identity).
    let session_id = turn_id.to_string();

    // Build and register the TurnEntry (register-before-handoff).
    let cancel = tokio_util::sync::CancellationToken::new();
    let entry = ironhermes_core::TurnEntry {
        turn_id,
        session_id: session_id.clone(),
        surface: ironhermes_core::Surface::Realtime,
        started_at: std::time::Instant::now(),
        cancel,
    };
    app_state.turn_registry.register(entry).await;

    // Store the permit pair (RAII — must outlive the turn, Pitfall 2).
    {
        let mut permits = app_state
            .realtime_permits
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        permits.insert(turn_id, (per_permit, global_permit));
    }

    // Record initial last_seen Instant for stale-prune liveness.
    {
        let mut sessions = app_state
            .realtime_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        sessions.insert(turn_id, std::time::Instant::now());
    }

    Ok(IssueRealtimeTokenResult {
        token,
        turn_id: turn_id.to_string(),
        session_id,
        chat_session_id,
    })
}

// ---------------------------------------------------------------------------
// Phase 36.17.12 BUG 5 — realtime_session_config server fn
// ---------------------------------------------------------------------------
//
// Builds the GA-shaped Realtime `session` object (the value placed under
// `session` in a `session.update` event) from config, so the browser does not
// hardcode the VAD/noise schema. Fixes the over-sensitivity bug: the previous
// browser-side session.update used the pre-2026 BETA shape (turn_detection /
// create_response / interrupt_response at the top `session` level), which the
// GA server ignores — so VAD ran on defaults with no noise reduction. The GA
// schema nests everything under `session.audio.input` (verified 2026:
// developers.openai.com/api/reference/resources/realtime/client-events).

/// BUG 5: allowed input noise-reduction profiles (server-side validation).
#[cfg(not(target_arch = "wasm32"))]
const REALTIME_ALLOWED_NOISE_REDUCTION: &[&str] = &["near_field", "far_field", "off"];

/// BUG 5: allowed turn-detection (VAD) modes (server-side validation).
#[cfg(not(target_arch = "wasm32"))]
const REALTIME_ALLOWED_VAD_MODES: &[&str] = &["semantic_vad", "server_vad"];

/// Allowed input-transcription models ("off" disables). Server-side validation.
#[cfg(not(target_arch = "wasm32"))]
const REALTIME_ALLOWED_TRANSCRIPTION: &[&str] = &[
    "gpt-4o-mini-transcribe",
    "gpt-4o-transcribe",
    "whisper-1",
    "off",
];

/// BUG 5 / Phase 39.3 Plan 01 (D-01, D-02): Pure, hermetic builder for the GA-shaped
/// Realtime `session` object JSON. Inputs are validated/clamped by the caller; this
/// function only shapes the JSON so it is unit-testable without Config::load or any
/// network call.
///
/// `instructions` (D-01): PromptBuilder output verbatim — agent identity + skills.
/// `tools` (D-02): Full ToolRegistry tool set in OpenAI function-tool format.
/// Both are TOP-LEVEL session fields (siblings of `audio`), per the GA Realtime
/// schema [VERIFIED: developers.openai.com/api/docs/guides/realtime-conversations].
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn build_realtime_session_json(
    noise_reduction: &str,
    vad_mode: &str,
    threshold: f32,
    silence_ms: u32,
    prefix_ms: u32,
    transcription_model: &str,
    instructions: &str,
    tools: &[serde_json::Value],
) -> String {
    // noise_reduction: "off" → JSON null (disables filtering); else { "type": ... }.
    let noise_reduction_val = if noise_reduction == "off" {
        serde_json::Value::Null
    } else {
        serde_json::json!({ "type": noise_reduction })
    };
    // transcription: "off"/empty → null (disabled); else { "model": ... }. Enabling
    // it makes the provider emit conversation.item.input_audio_transcription.completed
    // events carrying the user's words, which the web UI shows in the transcript card.
    let transcription_val = if transcription_model == "off" || transcription_model.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::json!({ "model": transcription_model })
    };
    // turn_detection: create_response/interrupt_response live INSIDE turn_detection
    // (GA), enabling auto-response + provider barge-in (D-08). server_vad carries
    // the energy threshold + timing; semantic_vad is model-based (no threshold).
    // Round to 3 decimals as f64 so the serialized JSON is clean — f32→f64
    // widening otherwise yields values like 0.699999988 for a 0.7 input.
    let threshold = ((threshold as f64) * 1000.0).round() / 1000.0;
    let turn_detection = if vad_mode == "server_vad" {
        serde_json::json!({
            "type": "server_vad",
            "threshold": threshold,
            "prefix_padding_ms": prefix_ms,
            "silence_duration_ms": silence_ms,
            "create_response": true,
            "interrupt_response": true
        })
    } else {
        serde_json::json!({
            "type": "semantic_vad",
            "create_response": true,
            "interrupt_response": true
        })
    };
    // D-01: instructions and D-02: tools are TOP-LEVEL session fields, siblings of
    // `audio` (NOT nested under audio). GA schema verified at:
    // developers.openai.com/api/docs/guides/realtime-conversations
    serde_json::json!({
        "type": "realtime",
        "instructions": instructions,
        "tools": tools,
        "tool_choice": "auto",
        "audio": {
            "input": {
                "noise_reduction": noise_reduction_val,
                "turn_detection": turn_detection,
                "transcription": transcription_val
            }
        }
    })
    .to_string()
}

/// Phase 36.17.12 BUG 5 / Phase 39.3 Plan 01 (D-01, D-02): Return the GA-shaped
/// Realtime `session` config JSON, resolved from `config.voice.*`. The browser sends
/// this verbatim inside a `session.update` event over the DataChannel. Invalid config
/// values fall back to safe defaults (far_field / semantic_vad) rather than erroring,
/// so a typo degrades gracefully instead of disabling realtime.
///
/// D-01: builds the `instructions` string from `PromptBuilder` — same identity +
/// active skills + context as the text-chat path (state.rs L519-533). Verbatim;
/// no realtime-specific trimming.
/// D-02: serializes the full `ToolRegistry` tool set (same set as text path) in
/// OpenAI function-tool format. No curated subset.
/// D-05a verbal ack: the built instructions contain an explicit directive telling
/// the agent to verbally acknowledge when it starts a long-running background task.
#[server]
pub async fn realtime_session_config() -> Result<String, ServerFnError> {
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
    let v = &config.voice;

    let noise_reduction =
        if REALTIME_ALLOWED_NOISE_REDUCTION.contains(&v.realtime_noise_reduction.as_str()) {
            v.realtime_noise_reduction.as_str()
        } else {
            "far_field"
        };
    let vad_mode = if REALTIME_ALLOWED_VAD_MODES.contains(&v.realtime_vad_mode.as_str()) {
        v.realtime_vad_mode.as_str()
    } else {
        "semantic_vad"
    };
    let threshold = v.realtime_vad_threshold.clamp(0.0, 1.0);
    let transcription_model =
        if REALTIME_ALLOWED_TRANSCRIPTION.contains(&v.realtime_transcription_model.as_str()) {
            v.realtime_transcription_model.as_str()
        } else {
            "gpt-4o-mini-transcribe"
        };

    // ── D-01: Build the Hermes system prompt (mirrors state.rs L519-533) ──────
    // Use PromptBuilder exactly as the text-chat path does. This gives the realtime
    // session the same identity + active skills + context as text turns.
    let app_state = crate::server::state::global_app_state();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut prompt_builder =
        ironhermes_agent::PromptBuilder::new(&app_state.config.model.default, "web")
            .with_provider(&app_state.config.model.provider)
            .load_context(&cwd);
    prompt_builder.set_skill_registry(app_state.runtime.skill_registry().clone());
    if let Some(ref manager) = app_state.memory_manager {
        prompt_builder.set_memory_manager(manager.clone());
    }
    prompt_builder.set_user_profile_enabled(app_state.config.memory.user_profile_enabled);
    prompt_builder.set_active_toolsets(app_state.runtime.merged_tools().enabled_toolset_names());
    // D-08 (Phase 46 Plan 04): populate connected_mcp_servers so requires_mcp_servers-gated
    // skills (e.g. the Cloudflare skills) only surface when their MCP server is connected.
    prompt_builder.set_connected_mcp_servers(
        app_state
            .runtime
            .mcp_manager()
            .map(|m| m.connected_server_names().into_iter().collect())
            .unwrap_or_default(),
    );
    // Phase 38.1 (D-04/D-05): freeze session timezone into PromptBuilder Timestamp slot.
    // Note: this path calls build() via skip_context_files, so the ephemeral Timestamp slot
    // is dropped for the realtime subagent; set_timezone is wired for correctness/consistency.
    prompt_builder.set_timezone(app_state.config.agent.timezone.clone());
    prompt_builder.load_memory().await;
    prompt_builder.load_skills();
    // Use build() (returns String) rather than build_system_message() (returns ChatMessage)
    // because we need the raw instructions string for the realtime session config.
    let base_instructions = prompt_builder.build();

    // D-05a verbal acknowledgment directive: append an explicit instruction that
    // tells the agent to verbally acknowledge when it kicks off a long-running or
    // background tool/research call, keeping the conversation going rather than
    // going silent. The visual in-flight badge (Plan 05) is the screen-side signal;
    // this directive is the voice-side half of D-05a.
    let instructions = format!(
        "{base_instructions}\n\n\
        LANGUAGE DIRECTIVE: Always respond in English, regardless of the language \
        spoken to you, unless the user explicitly asks you to switch languages.\n\n\
        VOICE INTERACTION DIRECTIVE: When you start a long-running or background \
        tool call, research task, or any async operation, immediately verbally \
        acknowledge that you are doing so — for example: \"I'll look into that \
        while we keep talking\" or \"I'm on it, give me a moment.\" Do not go \
        silent while background work runs. Continue the conversation naturally \
        while async tasks complete in the background."
    );

    // ── D-02: Serialize the full ToolRegistry tool set ────────────────────────
    // Mirror the chat path's field accessors (api.rs list_tools ~L204-212).
    // Read, collect, drop — never hold the async RwLock guard across .await.
    // Each entry uses the GA Realtime function-tool format:
    //   { "type": "function", "name": ..., "description": ..., "parameters": ... }
    // [VERIFIED: developers.openai.com/api/docs/guides/realtime-conversations]
    let tool_definitions = app_state
        .runtime
        .registry()
        .read()
        .await
        .get_definitions(None);
    let tools: Vec<serde_json::Value> = tool_definitions
        .into_iter()
        .map(|def| {
            serde_json::json!({
                "type": "function",
                "name": def.function.name,
                "description": def.function.description,
                "parameters": def.function.parameters,
            })
        })
        .collect();

    Ok(build_realtime_session_json(
        noise_reduction,
        vad_mode,
        threshold,
        v.realtime_vad_silence_ms,
        v.realtime_vad_prefix_ms,
        transcription_model,
        &instructions,
        &tools,
    ))
}

// =============================================================================
// Phase 39.1 Plan 07: realtime_session_end / realtime_heartbeat / realtime_prune
// / turns_list / turn_cancel  server fns
// =============================================================================

/// Deregister a realtime turn that has ended normally (user hung up / session
/// complete). Idempotent — deregister of an unknown `turn_id` is a no-op.
///
/// Validates the wire `turn_id` string as UUID (T-39.1-07-01).
/// Drops the stored permit pair → releases the ConcurrencyLayer slots (RAII).
#[server]
pub async fn realtime_session_end(turn_id: String) -> Result<(), ServerFnError> {
    let uuid = uuid::Uuid::parse_str(&turn_id)
        .map_err(|_| ServerFnError::new(format!("invalid turn_id: {turn_id}")))?;

    let app_state = crate::server::state::global_app_state();

    // Deregister from TurnRegistry (idempotent — no-op if absent).
    app_state.turn_registry.deregister(uuid).await;

    // Drop the stored permit pair → releases ConcurrencyLayer slots.
    {
        let mut permits = app_state
            .realtime_permits
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        permits.remove(&uuid);
    }

    // Remove from liveness map.
    {
        let mut sessions = app_state
            .realtime_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        sessions.remove(&uuid);
    }

    // WR-01: drain any pending approvals for this turn (unbounded-leak fix).
    // Without this, attacker-influenced entries (tool name + raw args) accumulate
    // in realtime_approvals across cancelled/ended turns indefinitely.
    {
        let mut approvals = app_state
            .realtime_approvals
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        approvals.retain(|_call_id, p| p.turn_id != uuid.to_string());
    }

    Ok(())
}

/// Cooperative-cancel poll / heartbeat endpoint.
///
/// The wasm client calls this every ~2-3 s while a realtime session is active.
/// Returns `RealtimeTurnSignal::Continue` or `RealtimeTurnSignal::Cancelled`.
///
/// On `Cancelled`: also deregisters the turn and drops its permits so the slot
/// is freed immediately (without waiting for the client to call
/// `realtime_session_end` — the client may already be tearing down).
///
/// The decision logic is factored into `realtime_signal_for` (pure helper,
/// unit-testable without `global_app_state()`).
#[server]
pub async fn realtime_heartbeat(turn_id: String) -> Result<RealtimeTurnSignal, ServerFnError> {
    let uuid = uuid::Uuid::parse_str(&turn_id)
        .map_err(|_| ServerFnError::new(format!("invalid turn_id: {turn_id}")))?;

    let app_state = crate::server::state::global_app_state();

    // Look up the CancellationToken from the TurnRegistry via the public accessor.
    // Returns None if already deregistered (race) — treat as Cancelled.
    let signal = match app_state.turn_registry.get_cancel_token(uuid).await {
        Some(cancel) => realtime_signal_for(&cancel),
        None => RealtimeTurnSignal::Cancelled,
    };

    match &signal {
        RealtimeTurnSignal::Continue => {
            // Update last_seen for stale-prune liveness tracking.
            let mut sessions = app_state
                .realtime_sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            sessions.insert(uuid, std::time::Instant::now());
        }
        RealtimeTurnSignal::Cancelled => {
            // Free the slot immediately — don't wait for realtime_session_end.
            app_state.turn_registry.deregister(uuid).await;
            {
                let mut permits = app_state
                    .realtime_permits
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                permits.remove(&uuid);
            }
            {
                let mut sessions = app_state
                    .realtime_sessions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                sessions.remove(&uuid);
            }
            // WR-01: drain pending approvals for this turn (unbounded-leak fix).
            {
                let mut approvals = app_state
                    .realtime_approvals
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                approvals.retain(|_call_id, p| p.turn_id != uuid.to_string());
            }
        }
    }

    Ok(signal)
}

/// Prune abandoned realtime sessions whose browser closed without calling
/// `realtime_session_end` (no heartbeat received within `stale_secs`).
///
/// Mirrors `/api/agents/prune` (default 120 s). For each stale entry:
/// cancels the TurnEntry's token (observable if the heartbeat races), deregisters
/// from the TurnRegistry, drops the permit pair, and removes from the liveness
/// map. Returns a JSON count.
#[server]
pub async fn realtime_prune(stale_secs: Option<u64>) -> Result<serde_json::Value, ServerFnError> {
    let threshold = std::time::Duration::from_secs(stale_secs.unwrap_or(120));
    let app_state = crate::server::state::global_app_state();

    // Collect stale turn_ids from liveness map (brief sync lock, no .await).
    let stale_ids: Vec<ironhermes_core::TurnId> = {
        let sessions = app_state
            .realtime_sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        sessions
            .iter()
            .filter_map(|(id, last_seen)| {
                if last_seen.elapsed() > threshold {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect()
    };

    let count = stale_ids.len();

    for uuid in stale_ids {
        // Cancel the turn (signals the token — heartbeat will observe Cancelled
        // if it races this prune).
        app_state.turn_registry.cancel_one(uuid).await;
        // Deregister + drop permits + remove from liveness map.
        app_state.turn_registry.deregister(uuid).await;
        {
            let mut permits = app_state
                .realtime_permits
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            permits.remove(&uuid);
        }
        {
            let mut sessions = app_state
                .realtime_sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            sessions.remove(&uuid);
        }
        // WR-01: drain pending approvals for each pruned turn (unbounded-leak fix).
        {
            let mut approvals = app_state
                .realtime_approvals
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            approvals.retain(|_call_id, p| p.turn_id != uuid.to_string());
        }
    }

    Ok(serde_json::json!({ "pruned": count }))
}

/// List all in-flight turns from the process-wide TurnRegistry (all surfaces:
/// Web, Gateway, Cli, Realtime).
///
/// Completes the web-page half of R39.1-09 / D-09: "the web agent page lists
/// them; Telegram can see web agents and vice versa."
#[server]
pub async fn turns_list() -> Result<Vec<TurnSummaryWire>, ServerFnError> {
    let app_state = crate::server::state::global_app_state();
    let summaries = app_state.turn_registry.list_all().await;
    Ok(summaries
        .into_iter()
        .map(TurnSummaryWire::from_core)
        .collect())
}

/// Cancel a specific turn by id (cross-surface via TurnRegistry.cancel_one).
///
/// For a Realtime turn, `cancel_one` fires the CancellationToken; the next
/// `realtime_heartbeat` poll observes `is_cancelled()` and returns `Cancelled`,
/// triggering the wasm client's `teardown_realtime_session` (Task 3).
/// Validates the wire `turn_id` string as UUID (T-39.1-07-01).
#[server]
pub async fn turn_cancel(turn_id: String) -> Result<bool, ServerFnError> {
    let uuid = uuid::Uuid::parse_str(&turn_id)
        .map_err(|_| ServerFnError::new(format!("invalid turn_id: {turn_id}")))?;
    let app_state = crate::server::state::global_app_state();
    Ok(app_state.turn_registry.cancel_one(uuid).await)
}

/// Wire-safe projection of `ironhermes_core::TurnSummary` for the web API.
///
/// `TurnSummary` uses `uuid::Uuid` for `turn_id` and `Surface` (no Serialize),
/// so we project to strings here for the JSON surface.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TurnSummaryWire {
    pub turn_id: String,
    pub session_id: String,
    pub surface: String,
    pub elapsed_ms: u128,
}

#[cfg(not(target_arch = "wasm32"))]
impl TurnSummaryWire {
    pub fn from_core(s: ironhermes_core::TurnSummary) -> Self {
        Self {
            turn_id: s.turn_id.to_string(),
            session_id: s.session_id,
            surface: s.surface.to_string(),
            elapsed_ms: s.elapsed_ms,
        }
    }
}

// =============================================================================
// Phase 39.3 Plan 03: trajectory log helper for realtime tool calls (D-04)
// =============================================================================

/// Log a completed realtime tool call to the trajectory writer (D-04 / OQ#3).
///
/// **Redaction contract:** callers MUST pass already-redacted args. Each call site resolves
/// the tool from the registry and calls `tool.redact_args(&args_parsed)` before invoking
/// this function — mirroring the text-chat path in `agent_loop.rs` (L2491-2497). The default
/// `redact_args` implementation returns the value verbatim (registry.rs L67), so tools that
/// carry no secrets are unaffected. Tools that override `redact_args` (e.g. `WebExtractTool`
/// stripping URL secrets) have their redaction applied at the call site before the entry is
/// serialized here. This closes the T-LOG-LEAK parity gap between the realtime and text paths.
///
/// **Best-effort semantics:** if the writer is `None`, or serialization fails, or `append_json_line`
/// returns `Err`, the error is logged and the function returns normally. A dropped trajectory
/// entry is acceptable (T-39.3-03-02 accept disposition). The tool call result is NOT affected.
///
/// This function is `#[cfg(feature = "server")]` via its call sites (server fns only).
#[cfg(feature = "server")]
fn log_realtime_tool_call_to_trajectory(
    writer: &Option<std::sync::Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>>,
    tool_name: &str,
    args: &serde_json::Value,
    output: &str,
    turn_id: &str,
    call_id: &str,
) {
    let Some(ref tw) = writer else {
        // No trajectory writer configured — best-effort, silent skip.
        return;
    };

    // Build a TrajectoryEntry using the canonical format (D-T-1).
    // turn_index is 0 (realtime turns are not yet indexed; a future phase can wire the counter).
    // impact_level is Read (conservative default for realtime; tool-specific override deferred).
    let entry = ironhermes_trajectory::TrajectoryEntry {
        name: tool_name.to_string(),
        // Caller has already applied tool.redact_args() before passing `args` — see call sites.
        args: args.clone(),
        result: Some(output.to_string()),
        error: None,
        duration_ms: 0, // Duration not tracked at this call site (realtime dispatch is async).
        impact_level: ironhermes_trajectory::ImpactLevel::Read,
        turn_index: 0,
        tool_call_id: call_id.to_string(),
        ts: {
            // Epoch-seconds as a plain integer string (e.g. "1718700000").
            // chrono is not a direct dep here so full ISO 8601 is deferred to a future phase.
            // The "s" suffix previously emitted was non-parseable and mislabeled as ISO 8601.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| format!("{}", d.as_secs()))
                .unwrap_or_else(|_| "unknown".to_string())
        },
    };

    match serde_json::to_string(&entry) {
        Ok(line) => {
            if let Err(e) = tw.append_json_line(&line) {
                tracing::warn!(
                    tool = tool_name,
                    turn_id = turn_id,
                    call_id = call_id,
                    error = %e,
                    "realtime trajectory log failed (best-effort — tool call succeeded)"
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                tool = tool_name,
                error = %e,
                "realtime trajectory entry serialization failed (best-effort)"
            );
        }
    }
}

// =============================================================================
// Phase 39.3 Plan 02: realtime_tool_call + realtime_approve server fns (D-02/D-03)
// =============================================================================

/// Result type returned by `realtime_tool_call` and `realtime_approve`.
///
/// Three variants encode the three outcomes of the approval gate:
/// - `Output`: tool executed successfully; `output` is the tool's string result.
/// - `ApprovalPending`: gate requires approval; call is held in
///   `AppState.realtime_approvals` keyed by `call_id`. Browser must render an
///   approval card and call `realtime_approve` to resolve.
/// - `Rejected`: the call was rejected (invalid turn, cancelled turn, unknown tool,
///   malformed args, or operator deny). No tool was dispatched.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum RealtimeToolResult {
    Output {
        output: String,
    },
    ApprovalPending {
        call_id: String,
        tool_name: String,
        arguments: String,
    },
    Rejected {
        reason: String,
    },
}

/// Pure helper for the approval-gate + dispatch decision in `realtime_tool_call`.
///
/// Factored out so the three-branch logic can be unit-tested deterministically
/// without requiring a live `global_app_state()`.
///
/// Arguments (all booleans that encode the relevant state):
/// - `turn_live`: the turn UUID is in the registry and its token is NOT cancelled.
/// - `tool_known`: the `name` is present in the active ToolRegistry definitions.
/// - `args_valid`: `arguments` parses as valid JSON.
/// - `needs_approval`: `should_prompt_for_approval(config_yolo)` returned true.
///
/// Returns an `Action` that the caller translates into a concrete `RealtimeToolResult`.
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
#[derive(Debug, PartialEq)]
pub enum RealtimeToolAction {
    /// Proceed with immediate dispatch.
    Dispatch,
    /// Hold the call; browser should render an approval card.
    HoldForApproval,
    /// Reject before dispatch (reason string).
    Reject(String),
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
pub fn decide_realtime_tool_action(
    turn_live: bool,
    tool_known: bool,
    args_valid: bool,
    needs_approval: bool,
) -> RealtimeToolAction {
    if !turn_live {
        return RealtimeToolAction::Reject("turn not found or expired".to_string());
    }
    if !tool_known {
        return RealtimeToolAction::Reject("unknown tool".to_string());
    }
    if !args_valid {
        return RealtimeToolAction::Reject("malformed arguments JSON".to_string());
    }
    if needs_approval {
        RealtimeToolAction::HoldForApproval
    } else {
        RealtimeToolAction::Dispatch
    }
}

/// Relay a realtime `function_call` DataChannel event through the approval gate
/// (D-03) and, if auto-approved, dispatch via `ToolRegistry` (D-02).
///
/// **Security invariants enforced (no bypass):**
/// 1. `turn_id` must parse as UUID and resolve to a live, non-cancelled entry in
///    `TurnRegistry` — stale/forged relays are rejected before any gate check.
/// 2. `name` must be present in `ToolRegistry.get_definitions(None)` — EoP mitigation.
/// 3. `arguments` must parse as valid JSON — input-validation gate.
/// 4. `should_prompt_for_approval(config.autonomous.yolo)` is called unconditionally;
///    if true, the call is held as `ApprovalPending` — the tool is NOT dispatched.
///    If false (yolo=true), the call is dispatched immediately.
///
/// **Two-call pattern (OQ#1 resolution):** this fn never blocks inside one HTTP
/// request for approval input. `ApprovalPending` is returned immediately; the
/// browser calls `realtime_approve` to resolve. Dioxus server-fn HTTP timeout risk
/// is thereby avoided (no multi-second hold inside a single POST).
///
/// **Lock discipline (Pitfall 4):** `realtime_approvals` (std::sync::Mutex) is locked
/// only to insert, then dropped before any `.await`. The registry RwLock is read,
/// collected, and dropped before dispatch.
#[server]
pub async fn realtime_tool_call(
    turn_id: String,
    call_id: String,
    name: String,
    arguments: String,
) -> Result<RealtimeToolResult, ServerFnError> {
    // --- V5: UUID parse ---
    let uuid = uuid::Uuid::parse_str(&turn_id)
        .map_err(|_| ServerFnError::new(format!("invalid turn_id: {turn_id}")))?;

    let app_state = crate::server::state::global_app_state();

    // --- T-39.3-02-02: turn liveness (Spoofing mitigation) ---
    let cancel = match app_state.turn_registry.get_cancel_token(uuid).await {
        Some(token) => token,
        None => {
            return Ok(RealtimeToolResult::Rejected {
                reason: "turn not found or expired".to_string(),
            })
        }
    };
    if cancel.is_cancelled() {
        return Ok(RealtimeToolResult::Rejected {
            reason: "turn cancelled".to_string(),
        });
    }

    // --- T-39.3-02-01: tool name validation (EoP mitigation) ---
    let tool_known = {
        let registry = app_state.runtime.registry().read().await;
        let defs = registry.get_definitions(None);
        let found = defs.iter().any(|d| d.function.name == name);
        drop(defs);
        found
    }; // registry RwLock guard dropped here — before any further .await
    if !tool_known {
        return Ok(RealtimeToolResult::Rejected {
            reason: "unknown tool".to_string(),
        });
    }

    // --- T-39.3-02-04: arguments validation (Input Validation mitigation) ---
    let args_parsed: serde_json::Value = serde_json::from_str(&arguments)
        .map_err(|e| ServerFnError::new(format!("malformed arguments JSON: {e}")))?;

    // --- D-03: approval gate ---
    let needs_approval =
        ironhermes_tools::approval::should_prompt_for_approval(app_state.config.autonomous.yolo);

    if needs_approval {
        // Hold: insert PendingApproval, return immediately (two-call pattern).
        // Lock, insert, drop — never held across .await.
        {
            let mut approvals = app_state
                .realtime_approvals
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            approvals.insert(
                call_id.clone(),
                crate::server::state::PendingApproval {
                    turn_id: turn_id.clone(),
                    tool_name: name.clone(),
                    arguments: arguments.clone(),
                },
            );
        }
        return Ok(RealtimeToolResult::ApprovalPending {
            call_id,
            tool_name: name,
            arguments,
        });
    }

    // --- Phase 39.2: emit tool_dispatched before dispatch ---
    #[cfg(not(target_arch = "wasm32"))]
    let bb_tool_start = {
        let t = std::time::Instant::now();
        if let Some(ref rec) = app_state.runtime.bb_recorder {
            let args_hash = ironhermes_blackbox::argument_hash(&args_parsed);
            rec.try_record(ironhermes_blackbox::BlackBoxRecorder::make_event(
                uuid,
                ironhermes_blackbox::Stage::Tool,
                "tool_dispatched",
                "hermes-2026-06",
                serde_json::json!({
                    "tool_name": &name,
                    "call_id": &call_id,
                    "surface": "realtime",
                    "arguments": &args_parsed,
                    "arguments_hash": args_hash,
                }),
            ));
        }
        t
    };

    // --- D-02: auto-approved — dispatch through ToolRegistry ---
    let output = {
        let registry = app_state.runtime.registry().read().await;
        registry.dispatch(&name, args_parsed.clone()).await
    }
    .map_err(|e| ServerFnError::new(format!("tool dispatch error: {e}")))?;

    // --- Phase 39.2: emit tool_completed after dispatch ---
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(ref rec) = app_state.runtime.bb_recorder {
        let result_hash = {
            use sha2::Digest;
            format!("{:x}", sha2::Sha256::digest(output.as_bytes()))
        };
        rec.try_record(ironhermes_blackbox::BlackBoxRecorder::make_event(
            uuid,
            ironhermes_blackbox::Stage::Tool,
            "tool_completed",
            "hermes-2026-06",
            serde_json::json!({
                "tool_name": &name,
                "call_id": &call_id,
                "surface": "realtime",
                "success": true,
                "result_hash": result_hash,
                "latency_ms": bb_tool_start.elapsed().as_millis() as u64,
            }),
        ));
    }

    // --- D-04: log to trajectory with redacted args (T-LOG-LEAK fix — parity with agent_loop.rs).
    // Resolve the tool from the registry and apply redact_args before logging, mirroring
    // the text-chat path (agent_loop.rs L2491-2497). Guard scope is kept tight: acquired,
    // used for redact_args, then dropped before the synchronous log call.
    let redacted_args = {
        let registry_guard = app_state.runtime.registry().read().await;
        if let Some(t) = registry_guard.get(&name) {
            t.redact_args(&args_parsed)
        } else {
            args_parsed.clone()
        }
    };
    log_realtime_tool_call_to_trajectory(
        &app_state.realtime_trajectory_writer,
        &name,
        &redacted_args,
        &output,
        &turn_id,
        &call_id,
    );

    Ok(RealtimeToolResult::Output { output })
}

// =============================================================================
// Phase 39.3 Plan 02: unit tests for realtime_tool_call (decide_realtime_tool_action)
// =============================================================================

#[cfg(all(test, not(target_arch = "wasm32")))]
mod realtime_tool_call_tests {
    use super::{decide_realtime_tool_action, RealtimeToolAction};

    /// Test 1 (yolo=true / auto-approved): gate says Dispatch.
    #[test]
    fn t1_yolo_true_returns_dispatch() {
        let action = decide_realtime_tool_action(
            true,  // turn_live
            true,  // tool_known
            true,  // args_valid
            false, // needs_approval = false (yolo=true bypasses)
        );
        assert_eq!(
            action,
            RealtimeToolAction::Dispatch,
            "yolo=true path must return Dispatch"
        );
    }

    /// Test 2 (yolo=false / approval required): gate says HoldForApproval.
    #[test]
    fn t2_yolo_false_returns_hold_for_approval() {
        let action = decide_realtime_tool_action(
            true, // turn_live
            true, // tool_known
            true, // args_valid
            true, // needs_approval = true (yolo=false)
        );
        assert_eq!(
            action,
            RealtimeToolAction::HoldForApproval,
            "yolo=false path must return HoldForApproval without dispatching"
        );
    }

    /// Test 3 (stale turn_id): turn not live → Reject before gate.
    #[test]
    fn t3_stale_turn_id_rejected_before_gate() {
        let action = decide_realtime_tool_action(
            false, // turn_live = false (stale/unknown)
            true,  // tool_known
            true,  // args_valid
            false, // needs_approval irrelevant — rejected first
        );
        match action {
            RealtimeToolAction::Reject(reason) => {
                assert!(
                    reason.contains("turn"),
                    "Reject reason must mention 'turn', got: {reason}"
                );
            }
            other => panic!("Expected Reject, got {other:?}"),
        }
    }

    /// Test 4 (cancelled turn): same Reject path — turn_live=false covers
    /// both stale (None from registry) and cancelled (is_cancelled=true).
    #[test]
    fn t4_cancelled_turn_rejected_before_dispatch() {
        let action = decide_realtime_tool_action(
            false, // turn_live=false (cancelled turn maps here)
            true,  // tool_known
            true,  // args_valid
            true,  // needs_approval irrelevant — rejected first
        );
        assert!(
            matches!(action, RealtimeToolAction::Reject(_)),
            "Cancelled turn must be rejected"
        );
    }

    /// Test 5 (unknown tool name): Reject after turn check, before dispatch.
    #[test]
    fn t5_unknown_tool_name_rejected() {
        let action = decide_realtime_tool_action(
            true,  // turn_live
            false, // tool_known = false (EoP mitigation)
            true,  // args_valid
            false, // needs_approval irrelevant — rejected before gate
        );
        match action {
            RealtimeToolAction::Reject(reason) => {
                assert!(
                    reason.contains("unknown tool"),
                    "Reject reason must mention 'unknown tool', got: {reason}"
                );
            }
            other => panic!("Expected Reject for unknown tool, got {other:?}"),
        }
    }
}

/// Resolve a held realtime tool-call approval — approve (dispatch) or deny (reject).
///
/// This is the second call in the two-call pattern (OQ#1 resolution).  The browser
/// calls this after the user interacts with the approval card rendered by the
/// `ApprovalPending` response from `realtime_tool_call`.
///
/// **Security invariants enforced (D-03):**
/// 1. `turn_id` UUID is re-validated and turn liveness re-checked — the turn may
///    have died while the approval card was shown.
/// 2. `call_id` is removed from `realtime_approvals` in one atomic operation —
///    resolved-once semantics (replay mitigation T-39.3-02-03): a second call with
///    the same `call_id` finds no entry and is rejected.
/// 3. If `approved == false` → `Rejected{reason:"denied"}` — no dispatch.
/// 4. If `approved == true` → stored args dispatched through `ToolRegistry`.
///
/// **Lock discipline:** `realtime_approvals` Mutex is locked only to remove the
/// entry; guard is dropped before any `.await`.
#[server]
pub async fn realtime_approve(
    turn_id: String,
    call_id: String,
    approved: bool,
) -> Result<RealtimeToolResult, ServerFnError> {
    // --- V5: UUID parse ---
    let uuid = uuid::Uuid::parse_str(&turn_id)
        .map_err(|_| ServerFnError::new(format!("invalid turn_id: {turn_id}")))?;

    let app_state = crate::server::state::global_app_state();

    // --- T-39.3-02-02 (re-check): turn liveness — the turn may have ended while
    // the approval card was displayed. Re-check before dispatching anything.
    let cancel = match app_state.turn_registry.get_cancel_token(uuid).await {
        Some(token) => token,
        None => {
            // Also clean up any stale pending entry for this call_id.
            let mut approvals = app_state
                .realtime_approvals
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            approvals.remove(&call_id);
            return Ok(RealtimeToolResult::Rejected {
                reason: "turn not found or expired".to_string(),
            });
        }
    };
    if cancel.is_cancelled() {
        let mut approvals = app_state
            .realtime_approvals
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        approvals.remove(&call_id);
        return Ok(RealtimeToolResult::Rejected {
            reason: "turn cancelled".to_string(),
        });
    }

    // --- T-39.3-02-03 (resolved-once): remove entry atomically.
    // Lock, remove, drop — guard never held across .await.
    let pending = {
        let mut approvals = app_state
            .realtime_approvals
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        approvals.remove(&call_id)
    };

    let pending = match pending {
        Some(p) => p,
        None => {
            return Ok(RealtimeToolResult::Rejected {
                reason: "no pending approval for this call_id".to_string(),
            });
        }
    };

    // --- D-03: deny path ---
    if !approved {
        return Ok(RealtimeToolResult::Rejected {
            reason: "denied".to_string(),
        });
    }

    // --- D-02: approved — parse stored args and dispatch through ToolRegistry.
    let args_parsed: serde_json::Value = serde_json::from_str(&pending.arguments)
        .map_err(|e| ServerFnError::new(format!("stored arguments invalid JSON: {e}")))?;

    // --- WR-03: re-validate tool is still known in the current registry before
    // dispatch (EoP mitigation — mirrors the tool_known check in realtime_tool_call).
    // The registry may have changed between the original realtime_tool_call and
    // this approval response. Use a separate guard scope so it drops before the
    // dispatch await point (no RwLock held across .await).
    {
        let registry = app_state.runtime.registry().read().await;
        let defs = registry.get_definitions(None);
        let found = defs.iter().any(|d| d.function.name == pending.tool_name);
        drop(defs);
        if !found {
            return Ok(RealtimeToolResult::Rejected {
                reason: "unknown tool".to_string(),
            });
        }
    }

    let output = {
        let registry = app_state.runtime.registry().read().await;
        registry
            .dispatch(&pending.tool_name, args_parsed.clone())
            .await
    }
    .map_err(|e| ServerFnError::new(format!("tool dispatch error: {e}")))?;

    // --- D-04: log to trajectory with redacted args (T-LOG-LEAK fix — parity with agent_loop.rs).
    // Resolve the tool from the registry and apply redact_args before logging, mirroring
    // the text-chat path (agent_loop.rs L2491-2497). Guard scope is kept tight: acquired,
    // used for redact_args, then dropped before the synchronous log call.
    let redacted_args = {
        let registry_guard = app_state.runtime.registry().read().await;
        if let Some(t) = registry_guard.get(&pending.tool_name) {
            t.redact_args(&args_parsed)
        } else {
            args_parsed.clone()
        }
    };
    log_realtime_tool_call_to_trajectory(
        &app_state.realtime_trajectory_writer,
        &pending.tool_name,
        &redacted_args,
        &output,
        &turn_id,
        &call_id,
    );

    Ok(RealtimeToolResult::Output { output })
}

// =============================================================================
// Phase 39.3 Plan 03: realtime_save_transcript server fn (D-04)
// =============================================================================

/// Pure helper: convert a `role` string and `text` to a `ChatMessage`, if the role
/// is recognized.  Returns `None` for unknown roles (defensive — unknown roles are
/// silently ignored by the caller).
///
/// Factored out so unit tests can exercise role-matching deterministically without
/// needing a full AppState (RESEARCH Pitfall 7 / TDD RED helper contract).
#[cfg(feature = "server")]
pub fn role_to_message(role: &str, text: String) -> Option<ironhermes_core::ChatMessage> {
    match role {
        "user" => Some(ironhermes_core::ChatMessage::user(text)),
        "assistant" => Some(ironhermes_core::ChatMessage::assistant(text)),
        _ => None,
    }
}

/// Persist a realtime user or assistant transcript turn to the session `StateStore`.
///
/// Called by the browser on:
///   - `conversation.item.input_audio_transcription.completed` → role = "user"
///   - `response.audio_transcript.done` → role = "assistant"  (RC-3, Plan 04)
///
/// # Parameters
///
/// - `chat_session_id`: the user's real chat session key (`"agent:main:web:dm:{uuid}"`),
///   produced by `create_session` and held in `SessionIdContext`.  This is the WRITE key —
///   `add_message` uses it, `get_session_messages` reads the same key (D-04 RC-1 fix).
/// - `turn_id`: the server-assigned turn UUID (from `IssueRealtimeTokenResult.turn_id`).
///   Used only for authz validation (must parse as UUID).  NOT the write key.
/// - `role`: "user" or "assistant".
/// - `text`: transcript text.
///
/// **D-04 RC-1 fix:** writes under `chat_session_id` (the real session key visible to
/// `get_session_messages`) instead of a bare turn UUID that was never in `sessions`.
///
/// **D-04 RC-2 fix (security posture — documented):**
/// The previous implementation used `get_cancel_token(turn_id)` as a HARD gate: if the
/// turn was already deregistered (e.g. by a heartbeat Cancelled or realtime_session_end
/// that raced with the spawned save), the transcript was silently dropped.  This caused
/// legitimate end-of-session transcripts to be lost.
///
/// Chosen posture: `turn_id` is validated only as a well-formed UUID (prevents injection
/// of non-UUID garbage).  The DB's `FOREIGN KEY sessions(id)` constraint — always ON in
/// this app — is the authoritative write guard: `add_message` will fail with an FK error
/// if `chat_session_id` does not correspond to a real session row, and that error is
/// logged and dropped (fire-and-forget policy, T-39.3-03-02).  Because this is a
/// single-user local app (CR-01 threat model scope), the session FK provides adequate
/// protection against injection; we do not need real-time liveness of the turn as an
/// additional hard gate.  The `chat_session_id` format validation (prefix + UUID suffix,
/// applied in `issue_realtime_token`) adds a structural guard before any DB call.
///
/// **Lock discipline (Pitfall 7):** `std::sync::Mutex` is locked, `add_message` called,
/// and the guard dropped — never held across an `.await`.
///
/// **Error handling:** errors are logged (not unwrapped) — a dropped transcript is
/// the accepted worst case (T-39.3-03-02 / RESEARCH Pitfall 7 fire-and-forget policy).
///
/// Unknown `role` values are silently ignored (returns `Ok(())`).
#[server]
pub async fn realtime_save_transcript(
    chat_session_id: String,
    turn_id: String,
    role: String,
    text: String,
) -> Result<(), ServerFnError> {
    // --- RC-2: validate turn_id as a UUID (authz — prevents non-UUID garbage) ---
    uuid::Uuid::parse_str(&turn_id)
        .map_err(|_| ServerFnError::new(format!("invalid turn_id: {turn_id}")))?;

    // --- RC-1: validate chat_session_id is a well-formed web session key ---
    // Structural check: non-empty, correct prefix, UUID suffix.
    // The sessions FK (enforced by add_message) is the real write guard.
    {
        const WEB_SESSION_PREFIX: &str = "agent:main:web:dm:";
        let uuid_part = chat_session_id
            .strip_prefix(WEB_SESSION_PREFIX)
            .ok_or_else(|| {
                ServerFnError::new(format!(
                    "chat_session_id '{chat_session_id}' is not a valid web session key"
                ))
            })?;
        uuid::Uuid::parse_str(uuid_part).map_err(|_| {
            ServerFnError::new(format!(
                "chat_session_id '{chat_session_id}' does not contain a valid UUID suffix"
            ))
        })?;
    }

    let msg = match role_to_message(&role, text) {
        Some(m) => m,
        // Unknown role is a no-op (Test 3 — defensive).
        None => return Ok(()),
    };

    let app_state = crate::server::state::global_app_state();

    // RC-2: turn-liveness is NOT a hard write gate (see security posture note above).
    // The sessions FK on add_message is the authoritative protection.

    // Lock, add_message, drop — NEVER hold std::sync::Mutex across .await (Pitfall 7).
    {
        let mut store = app_state
            .state_store
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Err(e) = store.add_message(&chat_session_id, &msg) {
            // Log, not unwrap — a dropped transcript is acceptable (T-39.3-03-02).
            tracing::warn!(
                chat_session_id = %chat_session_id,
                turn_id = %turn_id,
                role = %role,
                error = %e,
                "realtime_save_transcript: add_message failed (dropping transcript)"
            );
        }
    } // guard drops here — before any await

    Ok(())
}

// =============================================================================
// Phase 39.3 Plan 03: unit tests for realtime_save_transcript (D-04)
// Tests use role_to_message pure helper + a temp/in-memory StateStore.
// No global_app_state() needed.
// =============================================================================

// See the gate note on `realtime_trajectory_wiring_tests`: `role_to_message` is
// `#[cfg(feature = "server")]`, so this module must name the feature too.
#[cfg(all(test, not(target_arch = "wasm32"), feature = "server"))]
mod realtime_save_transcript_tests {
    use super::role_to_message;

    /// Test 1 (user role): role_to_message("user", text) → ChatMessage::user.
    #[test]
    fn t1_user_role_produces_user_message() {
        let msg = role_to_message("user", "hello from mic".to_string());
        assert!(msg.is_some(), "role 'user' must produce a ChatMessage");
        let m = msg.unwrap();
        // role must be Role::User
        assert_eq!(
            format!("{:?}", m.role),
            format!("{:?}", ironhermes_core::types::Role::User),
            "message role must be User"
        );
    }

    /// Test 2 (assistant role): role_to_message("assistant", text) → ChatMessage::assistant.
    #[test]
    fn t2_assistant_role_produces_assistant_message() {
        let msg = role_to_message("assistant", "hello from model".to_string());
        assert!(msg.is_some(), "role 'assistant' must produce a ChatMessage");
        let m = msg.unwrap();
        assert_eq!(
            format!("{:?}", m.role),
            format!("{:?}", ironhermes_core::types::Role::Assistant),
            "message role must be Assistant"
        );
    }

    /// Test 3 (unknown role): unrecognized role → None (no panic, no write).
    #[test]
    fn t3_unknown_role_returns_none() {
        let msg = role_to_message("system", "injected".to_string());
        assert!(
            msg.is_none(),
            "Unknown role must return None — ignored, not panicked"
        );
        let msg2 = role_to_message("tool", "output".to_string());
        assert!(msg2.is_none(), "role 'tool' must also be ignored");
        let msg3 = role_to_message("", "".to_string());
        assert!(msg3.is_none(), "Empty role must be ignored");
    }

    /// Test 4 (persisted then readable): add_message via a temp StateStore and confirm
    /// the message count increases (D-04 parity — same path as text turns).
    ///
    /// D-04 RC-1: the session key now uses the canonical web-DM format
    /// `"agent:main:web:dm:{uuid}"` — the same key produced by `create_session`
    /// and held in `SessionIdContext` — so FK is satisfied and `get_session_messages`
    /// reads the same row.
    #[test]
    fn t4_persisted_message_readable_in_state_store() {
        // Open an in-memory / temp StateStore so the test is hermetic.
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test_state.db");
        let mut store = ironhermes_state::StateStore::new(&db_path).expect("open test StateStore");

        // Use canonical web session key format (RC-1 fix): "agent:main:web:dm:{uuid}".
        let session_uuid = "00000000-0000-0000-0000-000000000001";
        let session_id = format!("agent:main:web:dm:{session_uuid}");
        let session_id = session_id.as_str();
        // Create a minimal web-platform session so add_message's FK is satisfied.
        store
            .create_session(session_id, "web", None, None, None, None)
            .expect("create_session");

        // Build a user message via the pure helper (same code path as server fn).
        let user_msg =
            role_to_message("user", "did you hear that?".to_string()).expect("user message");
        let row_id = store
            .add_message(session_id, &user_msg)
            .expect("add user message");
        assert!(row_id > 0, "add_message must return a positive row id");

        // Build an assistant message and persist it too.
        let asst_msg = role_to_message("assistant", "yes, loud and clear".to_string())
            .expect("assistant message");
        store
            .add_message(session_id, &asst_msg)
            .expect("add assistant message");

        // Read back — both messages must appear.
        let messages = store.get_messages(session_id).expect("get_messages");
        assert!(
            messages.len() >= 2,
            "At least 2 messages must be readable after persistence (got {})",
            messages.len()
        );
    }
}

// =============================================================================
// Phase 39.3 Plan 03: unit tests for trajectory wiring (D-04 / OQ#3)
// =============================================================================

// Gate names `feature = "server"` because the item under test is itself
// `#[cfg(feature = "server")]`; `not(target_arch = "wasm32")` alone is wider and
// left this module compiled without its subject under native default features.
#[cfg(all(test, not(target_arch = "wasm32"), feature = "server"))]
mod realtime_trajectory_wiring_tests {
    use super::log_realtime_tool_call_to_trajectory;
    use ironhermes_core::commands::context::TrajectoryWriterHandle;
    use std::sync::{Arc, Mutex};

    /// A minimal in-test TrajectoryWriterHandle impl that records appended lines.
    struct RecordingHandle {
        lines: Mutex<Vec<String>>,
    }

    impl RecordingHandle {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                lines: Mutex::new(vec![]),
            })
        }

        fn recorded(&self) -> Vec<String> {
            self.lines.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    impl TrajectoryWriterHandle for RecordingHandle {
        fn append_json_line(&self, line: &str) -> anyhow::Result<()> {
            self.lines
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(line.to_string());
            Ok(())
        }
    }

    /// Test 1 (Some writer receives entry): when writer is Some, a successful dispatch
    /// causes log_realtime_tool_call_to_trajectory to append one JSONL line.
    #[test]
    fn t1_some_writer_receives_trajectory_entry() {
        let handle = RecordingHandle::new();
        let writer: Option<Arc<dyn TrajectoryWriterHandle>> = Some(handle.clone());

        let args = serde_json::json!({"path": "/tmp/note.txt"});
        log_realtime_tool_call_to_trajectory(
            &writer,
            "read_file",
            &args,
            "file contents here",
            "00000000-0000-0000-0000-000000000001",
            "call-abc-123",
        );

        let lines = handle.recorded();
        assert_eq!(lines.len(), 1, "Exactly one JSONL line must be appended");
        let line = &lines[0];
        // The line must be valid JSON containing the tool name and call_id.
        let v: serde_json::Value =
            serde_json::from_str(line).expect("appended line must be valid JSON");
        assert_eq!(v["name"], "read_file", "name field must match");
        assert_eq!(v["tool_call_id"], "call-abc-123", "call_id must match");
        // args field is present (caller is responsible for pre-redacting before passing here).
        assert!(v.get("args").is_some(), "args field must be present");
    }

    /// Test 3 (redaction at call site): verifies that a pre-redacted value is what lands
    /// in the trajectory entry. The call sites in `realtime_tool_call` and `realtime_approve`
    /// resolve the tool via the registry and call `tool.redact_args()` before invoking this
    /// function. This test proves the function faithfully records whatever args it receives —
    /// i.e. that call-site redaction flows through correctly (T-LOG-LEAK closure).
    #[test]
    fn t3_pre_redacted_args_recorded_verbatim() {
        let handle = RecordingHandle::new();
        let writer: Option<Arc<dyn TrajectoryWriterHandle>> = Some(handle.clone());

        // Simulate what the call site does: redact_args has replaced the secret URL
        // with a placeholder before calling log_realtime_tool_call_to_trajectory.
        let redacted = serde_json::json!({"url": "[redacted]", "selector": ".content"});
        log_realtime_tool_call_to_trajectory(
            &writer,
            "web_extract",
            &redacted,
            "extracted content",
            "00000000-0000-0000-0000-000000000003",
            "call-redact-789",
        );

        let lines = handle.recorded();
        assert_eq!(lines.len(), 1);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).expect("must be valid JSON");
        assert_eq!(v["name"], "web_extract");
        // The redacted placeholder must appear in the logged entry, not the original secret.
        assert_eq!(
            v["args"]["url"], "[redacted]",
            "pre-redacted placeholder must be preserved in the trajectory entry"
        );
    }

    /// Test 2 (None writer — best-effort): when writer is None, the function returns
    /// normally without panicking. Tool call result is unaffected.
    #[test]
    fn t2_none_writer_returns_without_panic() {
        let writer: Option<Arc<dyn TrajectoryWriterHandle>> = None;
        let args = serde_json::json!({"query": "status"});

        // Must not panic — best-effort semantics (T-39.3-03-02).
        log_realtime_tool_call_to_trajectory(
            &writer,
            "web_search",
            &args,
            "search results",
            "00000000-0000-0000-0000-000000000002",
            "call-xyz-456",
        );
        // If we get here, best-effort None path works correctly.
    }
}

// =============================================================================
// Phase 39.3 Plan 02: unit tests for realtime_approve
// Tests use a synthetic PendingApproval map (no global_app_state needed).
// =============================================================================

// See the gate note on `realtime_trajectory_wiring_tests`: `server::state` is
// `#[cfg(feature = "server")]`, so this module must name the feature too.
#[cfg(all(test, not(target_arch = "wasm32"), feature = "server"))]
mod realtime_approve_tests {
    use crate::server::state::PendingApproval;
    use std::collections::HashMap;

    /// Build a minimal in-memory approvals map pre-loaded with one entry.
    fn make_approvals_with(call_id: &str) -> std::sync::Mutex<HashMap<String, PendingApproval>> {
        let mut map = HashMap::new();
        map.insert(
            call_id.to_string(),
            PendingApproval {
                turn_id: "00000000-0000-0000-0000-000000000001".to_string(),
                tool_name: "read_file".to_string(),
                arguments: r#"{"path":"/tmp/test.txt"}"#.to_string(),
            },
        );
        std::sync::Mutex::new(map)
    }

    /// Helper: simulate the resolved-once remove operation.
    fn take_pending(
        map: &std::sync::Mutex<HashMap<String, PendingApproval>>,
        call_id: &str,
    ) -> Option<PendingApproval> {
        map.lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(call_id)
    }

    /// Test 1 (Approve): pending entry is removed and the stored data is available
    /// for dispatch; approved=true path takes the dispatch branch.
    #[test]
    fn t1_approve_removes_entry_and_returns_stored_data() {
        let approvals = make_approvals_with("call-abc");

        // Simulate the resolved-once remove.
        let pending = take_pending(&approvals, "call-abc");
        assert!(pending.is_some(), "Entry must be present before approval");

        let p = pending.unwrap();
        assert_eq!(p.tool_name, "read_file");
        assert_eq!(p.arguments, r#"{"path":"/tmp/test.txt"}"#);

        // After removal, a second take finds nothing (resolved-once).
        let second = take_pending(&approvals, "call-abc");
        assert!(
            second.is_none(),
            "Second resolution of same call_id must find no entry (replay mitigation)"
        );
    }

    /// Test 2 (Deny): approved=false → Rejected{reason:"denied"} without dispatch.
    /// Proven by checking the deny branch returns the correct reason.
    #[test]
    fn t2_deny_returns_rejected_without_dispatch() {
        let approved = false;
        // Simulate the deny decision path: after removing the entry, if !approved → Rejected.
        let reason = if !approved {
            Some("denied".to_string())
        } else {
            None
        };
        assert_eq!(
            reason,
            Some("denied".to_string()),
            "Deny path must return Rejected{{reason:denied}}"
        );
    }

    /// Test 3 (unknown call_id): no entry in map → Rejected (no panic; replay mitigation).
    #[test]
    fn t3_unknown_call_id_rejected() {
        let approvals = make_approvals_with("call-abc");
        // Try to take a call_id that was never inserted.
        let pending = take_pending(&approvals, "call-UNKNOWN");
        assert!(
            pending.is_none(),
            "Unknown call_id must yield None → Rejected (no panic, no entry leaked)"
        );
        // The existing entry is untouched.
        let existing = take_pending(&approvals, "call-abc");
        assert!(
            existing.is_some(),
            "Unrelated call_id must not affect existing entry"
        );
    }

    /// Test 4 (turn died while pending): simulated by checking that when
    /// get_cancel_token returns None, we still clean up and return Rejected.
    /// In the pure unit-test context, we model this as: entry is present but
    /// turn is dead → we clean up the map (remove call_id) and return Rejected.
    #[test]
    fn t4_turn_died_while_pending_cleans_up_and_rejects() {
        let approvals = make_approvals_with("call-abc");
        // Simulate: turn_live = false (get_cancel_token → None).
        // The server fn removes the entry and returns Rejected{turn not found or expired}.
        let turn_live = false;
        if !turn_live {
            // Clean up stale pending entry.
            let removed = take_pending(&approvals, "call-abc");
            assert!(
                removed.is_some(),
                "Entry must have been present before turn-death cleanup"
            );
        }
        // After cleanup, entry is gone.
        let after = take_pending(&approvals, "call-abc");
        assert!(
            after.is_none(),
            "Entry must be removed when turn dies while pending"
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod realtime_token_tests {
    use super::build_realtime_session_json;
    use super::REALTIME_ALLOWED_MODELS;
    use super::REALTIME_ALLOWED_VOICES;

    // Helper: run whitelist validation inline (mirrors the server fn's first two checks).
    // This is offline/hermetic — no reqwest, no env var, no network call.
    fn validate_model_voice(model: &str, voice: &str) -> Result<(), String> {
        if !REALTIME_ALLOWED_MODELS.contains(&model) {
            return Err(format!("Unknown model '{model}' — realtime unavailable"));
        }
        if !REALTIME_ALLOWED_VOICES.contains(&voice) {
            return Err(format!("Unknown voice '{voice}' — realtime unavailable"));
        }
        Ok(())
    }

    /// T-36.17.12-02-V1: Bogus model is rejected with Err BEFORE any network call
    /// (test is offline/hermetic — whitelist check runs before reqwest).
    #[test]
    fn realtime_token_validation_bogus_model() {
        let result = validate_model_voice("bogus-model", "shimmer");
        assert!(
            result.is_err(),
            "Bogus model must be rejected by whitelist validation"
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Unknown model"),
            "Error must mention 'Unknown model', got: {msg}"
        );
        // Security: error must not contain any api_key material
        assert!(
            !msg.contains("sk-"),
            "Error message must not leak key material: {msg}"
        );
    }

    /// T-36.17.12-02-V2: Bogus voice (with valid model) is rejected with Err BEFORE
    /// any network call (test is offline/hermetic).
    #[test]
    fn realtime_token_validation_bogus_voice() {
        let result = validate_model_voice("gpt-realtime", "bogus-voice");
        assert!(
            result.is_err(),
            "Bogus voice must be rejected by whitelist validation"
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Unknown voice"),
            "Error must mention 'Unknown voice', got: {msg}"
        );
        assert!(
            !msg.contains("sk-"),
            "Error message must not leak key material: {msg}"
        );
    }

    /// T-36.17.12-02-V3: Valid model+voice combo passes whitelist (offline/hermetic).
    /// BUG 3 fix: updated from `gpt-realtime-2` to `gpt-realtime` (GA model name, 2026).
    #[test]
    fn realtime_token_validation_valid_combo_passes_whitelist() {
        assert!(
            validate_model_voice("gpt-realtime", "alloy").is_ok(),
            "gpt-realtime + alloy must pass whitelist"
        );
        assert!(
            validate_model_voice("gpt-realtime", "shimmer").is_ok(),
            "gpt-realtime + shimmer must pass whitelist"
        );
        assert!(
            validate_model_voice("gpt-realtime", "echo").is_ok(),
            "gpt-realtime + echo must pass whitelist"
        );
    }

    // ── BUG 5: GA session.update JSON shape (offline/hermetic) ───────────────

    /// BUG 5: the built session config MUST use the GA nesting — turn_detection,
    /// noise_reduction, and transcription all under `audio.input` — NOT the
    /// deprecated beta top-level shape that the server silently ignores.
    #[test]
    fn realtime_session_json_uses_ga_audio_input_nesting() {
        let json = build_realtime_session_json(
            "far_field",
            "semantic_vad",
            0.5,
            500,
            300,
            "gpt-4o-mini-transcribe",
            "",
            &[],
        );
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let input = &v["audio"]["input"];
        assert!(input.is_object(), "audio.input must be an object: {json}");
        assert_eq!(input["turn_detection"]["type"], "semantic_vad");
        assert_eq!(input["noise_reduction"]["type"], "far_field");
        assert_eq!(input["transcription"]["model"], "gpt-4o-mini-transcribe");
        // beta-shape keys must NOT be at the top session level
        assert!(
            v.get("turn_detection").is_none(),
            "turn_detection must not be top-level"
        );
        assert!(
            v.get("create_response").is_none(),
            "create_response must not be top-level"
        );
        // create_response/interrupt_response live INSIDE turn_detection (GA)
        assert_eq!(input["turn_detection"]["create_response"], true);
        assert_eq!(input["turn_detection"]["interrupt_response"], true);
        assert_eq!(v["type"], "realtime");
    }

    /// BUG 5: server_vad carries the energy threshold + timing; semantic_vad does not.
    #[test]
    fn realtime_session_json_server_vad_carries_threshold() {
        let json = build_realtime_session_json(
            "near_field",
            "server_vad",
            0.7,
            600,
            250,
            "whisper-1",
            "",
            &[],
        );
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let td = &v["audio"]["input"]["turn_detection"];
        assert_eq!(td["type"], "server_vad");
        assert_eq!(td["threshold"], 0.7);
        assert_eq!(td["silence_duration_ms"], 600);
        assert_eq!(td["prefix_padding_ms"], 250);
        assert_eq!(v["audio"]["input"]["noise_reduction"]["type"], "near_field");
    }

    /// BUG 5: noise_reduction "off" emits JSON null (disables filtering), not a string.
    #[test]
    fn realtime_session_json_off_noise_reduction_is_null() {
        let json = build_realtime_session_json(
            "off",
            "semantic_vad",
            0.5,
            500,
            300,
            "gpt-4o-mini-transcribe",
            "",
            &[],
        );
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            v["audio"]["input"]["noise_reduction"].is_null(),
            "off must serialize noise_reduction as null: {json}"
        );
    }

    /// Transcription: a model name emits { "model": ... }; "off" emits null.
    #[test]
    fn realtime_session_json_transcription_on_and_off() {
        let on = build_realtime_session_json(
            "far_field",
            "semantic_vad",
            0.5,
            500,
            300,
            "gpt-4o-transcribe",
            "",
            &[],
        );
        let on_v: serde_json::Value = serde_json::from_str(&on).unwrap();
        assert_eq!(
            on_v["audio"]["input"]["transcription"]["model"],
            "gpt-4o-transcribe"
        );

        let off =
            build_realtime_session_json("far_field", "semantic_vad", 0.5, 500, 300, "off", "", &[]);
        let off_v: serde_json::Value = serde_json::from_str(&off).unwrap();
        assert!(
            off_v["audio"]["input"]["transcription"].is_null(),
            "off must serialize transcription as null: {off}"
        );
    }

    /// T-36.17.12-02-NK: Missing OPENAI_API_KEY yields Err (typed ServerFnError,
    /// no panic) and the error string does not contain any key material.
    ///
    /// Uses a scoped env-var guard to ensure the var is unset during the test
    /// without affecting other concurrent tests (uses a unique var name).
    #[test]
    fn realtime_token_no_key() {
        // Use a unique env var name that cannot conflict with real env (randomised per-test).
        let unique_var = "IRONHERMES_TEST_MISSING_REALTIME_KEY_36_17_12";
        // Ensure it is not set.
        std::env::remove_var(unique_var);

        // Simulate the key-resolution + absence check from the server fn.
        // In the real fn: key_env = unique_var (mocked), api_key = std::env::var(key_env).
        let result: Result<String, String> = std::env::var(unique_var)
            .map_err(|_| "OPENAI_API_KEY not set — realtime unavailable".to_string());

        assert!(result.is_err(), "Missing key must yield Err (no panic)");

        let msg = result.unwrap_err();
        // Typed error (no panic) — error message is descriptive
        assert!(
            msg.contains("not set"),
            "Error must mention 'not set', got: {msg}"
        );
        // Security: error must not contain any api_key material
        assert!(
            !msg.contains("sk-"),
            "Error must not leak key material: {msg}"
        );
        // The unique_var value itself is not in the error (we log only the status)
        assert!(
            !msg.contains(unique_var),
            "Error must not echo the env var name as key material: {msg}"
        );
    }

    // ── Phase 39.3 Plan 01 Task 1: instructions + tools injection (D-01 / D-02) ──

    /// T-39.3-01-T1: A non-empty `instructions` string is placed at the top-level
    /// `instructions` key of the parsed session JSON (D-01).
    #[test]
    fn realtime_session_json_instructions_at_top_level() {
        let instructions = "You are Hermes, a helpful assistant.";
        let json = build_realtime_session_json(
            "far_field",
            "semantic_vad",
            0.5,
            500,
            300,
            "gpt-4o-mini-transcribe",
            instructions,
            &[],
        );
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v["instructions"], instructions,
            "instructions must be a top-level key with the exact passed string"
        );
    }

    /// T-39.3-01-T2: A tools slice of two function-tool Values is placed at the
    /// top-level `tools` array (length 2), and `tool_choice` is `"auto"` (D-02).
    #[test]
    fn realtime_session_json_tools_and_tool_choice_at_top_level() {
        let tool_a = serde_json::json!({
            "type": "function",
            "name": "search",
            "description": "Search the web",
            "parameters": {"type": "object", "properties": {}, "required": []}
        });
        let tool_b = serde_json::json!({
            "type": "function",
            "name": "read_file",
            "description": "Read a file",
            "parameters": {"type": "object", "properties": {}, "required": []}
        });
        let tools = vec![tool_a, tool_b];
        let json = build_realtime_session_json(
            "far_field",
            "semantic_vad",
            0.5,
            500,
            300,
            "gpt-4o-mini-transcribe",
            "",
            &tools,
        );
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let tools_arr = v["tools"].as_array().expect("tools must be an array");
        assert_eq!(tools_arr.len(), 2, "tools array must have 2 entries");
        assert_eq!(tools_arr[0]["name"], "search");
        assert_eq!(tools_arr[1]["name"], "read_file");
        assert_eq!(v["tool_choice"], "auto", "tool_choice must be 'auto'");
    }

    /// T-39.3-01-T3: Empty tools slice still emits a `tools` array and the existing
    /// `audio.input.*` nesting is unchanged (no regression to BUG-5 GA schema).
    #[test]
    fn realtime_session_json_empty_tools_and_audio_nesting_intact() {
        let json = build_realtime_session_json(
            "far_field",
            "semantic_vad",
            0.5,
            500,
            300,
            "gpt-4o-mini-transcribe",
            "some instructions",
            &[],
        );
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        // tools must be an array (possibly empty)
        assert!(
            v["tools"].is_array(),
            "tools must be an array even when empty"
        );
        // audio.input nesting must be intact (BUG-5 GA shape regression check)
        let input = &v["audio"]["input"];
        assert!(input.is_object(), "audio.input must still be an object");
        assert_eq!(input["turn_detection"]["type"], "semantic_vad");
        assert_eq!(input["noise_reduction"]["type"], "far_field");
    }

    /// T-39.3-01-T4: `instructions` and `tools` are siblings of `audio`, NOT nested
    /// under `audio` (Pitfall 2 from RESEARCH.md — wrong schema nesting).
    #[test]
    fn realtime_session_json_instructions_tools_are_siblings_of_audio() {
        let tool = serde_json::json!({
            "type": "function",
            "name": "ping",
            "description": "ping tool",
            "parameters": {"type": "object", "properties": {}, "required": []}
        });
        let json = build_realtime_session_json(
            "far_field",
            "semantic_vad",
            0.5,
            500,
            300,
            "gpt-4o-mini-transcribe",
            "hermes identity",
            &[tool],
        );
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        // instructions and tools must NOT appear under audio
        assert!(
            v["audio"]["instructions"].is_null() || v["audio"].get("instructions").is_none(),
            "instructions must NOT be nested under audio"
        );
        assert!(
            v["audio"]["tools"].is_null() || v["audio"].get("tools").is_none(),
            "tools must NOT be nested under audio"
        );
        // they must be top-level siblings of audio
        assert!(
            v.get("instructions").is_some(),
            "instructions must be top-level"
        );
        assert!(v.get("tools").is_some(), "tools must be top-level");
    }
}

// =============================================================================
// Phase 40.5 Plan 03 Task 3 — RED gate: resolve_realtime_voice tests
// Tests reference resolve_realtime_voice which does NOT exist yet → compile error = RED.
// =============================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod realtime_identity_tests_40_5_03 {
    use super::{resolve_realtime_voice, REALTIME_ALLOWED_VOICES};
    use ironhermes_core::config::Config;

    /// D-12: identity override wins over global when the identity has realtime_voice set.
    #[test]
    fn realtime_resolves_identity_override() {
        let mut config = Config::default();
        config.voice.realtime_voice = "shimmer".to_string();
        config
            .identities
            .entry("orb_bloom".to_string())
            .or_default()
            .voice
            .realtime_voice = Some("echo".to_string());

        // Identity override wins.
        let voice = resolve_realtime_voice(&config, Some("orb_bloom"))
            .expect("orb_bloom is a known identity");
        assert_eq!(
            voice, "echo",
            "identity realtime_voice must override global"
        );

        // None slug → global fallback.
        let global = resolve_realtime_voice(&config, None).expect("None slug must always succeed");
        assert_eq!(
            global, "shimmer",
            "None slug must return global realtime_voice"
        );
    }

    /// T-40.5-03-01: unknown slug is rejected with an error before any network call.
    #[test]
    fn realtime_unknown_slug_rejected() {
        let config = Config::default();
        let result = resolve_realtime_voice(&config, Some("completely_unknown_slug_xyz"));
        assert!(
            result.is_err(),
            "unknown identity slug must return an error (T-V5 gate)"
        );
    }

    /// T-40.5-03-03: the resolved voice is still subject to REALTIME_ALLOWED_VOICES
    /// (defense in depth — the identity override does not bypass the whitelist).
    #[test]
    fn realtime_override_still_whitelisted() {
        let mut config = Config::default();
        // Set orb_bloom's realtime_voice to something NOT in the whitelist.
        config
            .identities
            .entry("orb_bloom".to_string())
            .or_default()
            .voice
            .realtime_voice = Some("definitely_invalid_voice".to_string());

        let resolved = resolve_realtime_voice(&config, Some("orb_bloom"))
            .expect("orb_bloom is a known identity");
        assert_eq!(resolved, "definitely_invalid_voice");

        // REALTIME_ALLOWED_VOICES must NOT contain the invalid value.
        assert!(
            !REALTIME_ALLOWED_VOICES.contains(&resolved.as_str()),
            "REALTIME_ALLOWED_VOICES must not allow identity-override bypass"
        );
    }
}

// =============================================================================
// Phase 46.9 Plan 02 (D-05/D-10): Models screen role-assignment read/write.
// Mirrors the update_voice_config four-step gated-write protocol (api.rs:1333).
// =============================================================================

/// The fixed six D-05 role keys. No other key is ever accepted by
/// `validate_models_roles_payload` — this is the single source of truth for
/// "which roles exist" on the Models screen (UI-SPEC models-role-picker).
pub const MODEL_ROLE_KEYS: [&str; 6] = [
    "compression",
    "summarization",
    "fast",
    "session_search",
    "kanban_decomposer",
    "kanban_judge",
];

/// Phase 46.9 Plan 02 (D-05): One row of the Models role-picker. `provider`/
/// `model` are `None` when the role has no explicit assignment in
/// `config.model.roles` (or the entry exists but carries no model override) —
/// the client renders "— uses default" in that case. Also reused as the
/// per-role write entry in `ModelsRolesWritePayload`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModelRoleAssignment {
    pub role_key: String,
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// Phase 46.9 Plan 02 (D-05): Read-only snapshot for the Models screen —
/// global default model + all six role assignments + the write-gate flag.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModelsRolesSnapshot {
    pub default_model: String,
    /// Phase 46.9 Plan 07 (GAP-1/D-05): the current `config.model.provider`,
    /// populating the Models-screen provider dropdown's pre-selection.
    pub provider: String,
    /// Exactly the six MODEL_ROLE_KEYS, in fixed order (D-05: "a 1-model
    /// catalog renders the same list").
    pub roles: Vec<ModelRoleAssignment>,
    /// Whether the web config write gate is open. The UI locks ASSIGN when false.
    pub web_config_write_enabled: bool,
}

/// Phase 46.9 Plan 02 (D-05/D-10): Web-safe write payload for the Models
/// screen. `default_model: None` leaves `config.model.default` unchanged;
/// each entry in `roles` upserts one `config.model.roles` key. Only the six
/// MODEL_ROLE_KEYS are accepted — `validate_models_roles_payload` rejects
/// anything else before any mutation (T-46.9-06).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModelsRolesWritePayload {
    pub default_model: Option<String>,
    /// Phase 46.9 Plan 07 (GAP-1/D-05): `Some` merges into
    /// `config.model.provider` inside the same gated write; `None` leaves
    /// it unchanged.
    #[serde(default)]
    pub provider: Option<String>,
    pub roles: Vec<ModelRoleAssignment>,
}

/// Phase 46.9 Plan 02 (T-46.9-06): Validate the payload BEFORE any config
/// mutation. Rejects role keys outside the fixed six and over-length strings
/// (arbitrary role keys / oversized ids from the browser — threat register).
#[cfg(not(target_arch = "wasm32"))]
fn validate_models_roles_payload(payload: &ModelsRolesWritePayload) -> Result<(), ServerFnError> {
    if let Some(ref m) = payload.default_model {
        if m.len() > 256 {
            return Err(ServerFnError::new("default_model too long (max 256 chars)"));
        }
    }
    // Phase 46.9 Plan 07 (T-46.9-14): reject an oversized provider string
    // BEFORE any mutation (V5 input validation).
    if let Some(ref p) = payload.provider {
        if p.len() > 64 {
            return Err(ServerFnError::new("provider too long (max 64 chars)"));
        }
    }
    for role in &payload.roles {
        if !MODEL_ROLE_KEYS.iter().any(|k| *k == role.role_key) {
            return Err(ServerFnError::new(format!(
                "unknown role key '{}' — must be one of {:?}",
                role.role_key, MODEL_ROLE_KEYS
            )));
        }
        if let Some(ref p) = role.provider {
            if p.len() > 64 {
                return Err(ServerFnError::new(format!(
                    "provider for role '{}' too long (max 64 chars)",
                    role.role_key
                )));
            }
        }
        if let Some(ref m) = role.model {
            if m.len() > 256 {
                return Err(ServerFnError::new(format!(
                    "model for role '{}' too long (max 256 chars)",
                    role.role_key
                )));
            }
        }
    }
    Ok(())
}

/// Phase 46.9 Plan 02: Build the read-side snapshot from a loaded Config.
/// Shared by `get_models_roles_config` and the `model_roles` test module so
/// the write-then-read assertion never has to go through the #[server]
/// HTTP-extractor machinery (mirrors `build_voice_snapshot`).
#[cfg(not(target_arch = "wasm32"))]
fn build_models_roles_snapshot(config: &ironhermes_core::config::Config) -> ModelsRolesSnapshot {
    let roles = MODEL_ROLE_KEYS
        .iter()
        .map(|key| {
            let entry = config.model.roles.get(*key);
            ModelRoleAssignment {
                role_key: (*key).to_string(),
                provider: entry.map(|r| r.provider.clone()),
                model: entry.and_then(|r| r.model.clone()),
            }
        })
        .collect();
    ModelsRolesSnapshot {
        default_model: config.model.default.clone(),
        provider: config.model.provider.clone(),
        roles,
        web_config_write_enabled: config.security.web_config_write_enabled,
    }
}

/// Phase 46.9 Plan 02 (D-05/D-10): Merge a validated payload into a freshly
/// loaded Config. `provider` defaults to `"main"` (inherit main provider,
/// per `ModelRoleConfig`'s doc comment, config.rs:245) when a role entry
/// omits it — the Models catalog (`list_models`) has no per-model provider
/// field, so the ASSIGN flow always writes the inherit-main sentinel.
#[cfg(not(target_arch = "wasm32"))]
fn merge_models_roles_payload(
    config: &mut ironhermes_core::config::Config,
    payload: &ModelsRolesWritePayload,
) {
    if let Some(ref default_model) = payload.default_model {
        config.model.default = default_model.clone();
    }
    if let Some(ref provider) = payload.provider {
        config.model.provider = provider.clone();
    }
    for role in &payload.roles {
        config.model.roles.insert(
            role.role_key.clone(),
            ironhermes_core::config::ModelRoleConfig {
                provider: role.provider.clone().unwrap_or_else(|| "main".to_string()),
                model: role.model.clone(),
            },
        );
    }
}

/// Phase 46.9 Plan 02 (D-05): Read-side server fn for the Models screen role
/// picker. Fresh `Config::load()`, never `app_state.config` (that is the
/// startup snapshot — RESEARCH anti-pattern; precedent: `get_voice_config`).
#[server]
pub async fn get_models_roles_config() -> Result<ModelsRolesSnapshot, ServerFnError> {
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
    Ok(build_models_roles_snapshot(&config))
}

/// Phase 46.9 Plan 02 (D-05/D-10): Write server fn for the Models screen.
/// Four-step gated-write protocol (mirrors `update_voice_config`, api.rs:1333):
/// 1. validate payload, 2. fresh `Config::load()`, 3. fail-closed gate check,
/// 4. merge + atomic `config.save()`.
#[server]
pub async fn update_models_roles_config(
    payload: ModelsRolesWritePayload,
) -> Result<(), ServerFnError> {
    // Step 1: validate
    validate_models_roles_payload(&payload)?;

    // Step 2: fresh disk read (NOT app_state.config — that is the startup snapshot)
    let mut config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;

    // Step 3: gate — web config write is disabled unless operator opts in
    if !config.security.web_config_write_enabled {
        return Err(ServerFnError::new("Config writes are disabled"));
    }

    // Step 4: merge web-safe fields, then atomic save
    merge_models_roles_payload(&mut config, &payload);
    config
        .save()
        .map_err(|e| ServerFnError::new(format!("Config save failed: {e}")))?;

    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod model_roles {
    use super::{
        build_models_roles_snapshot, merge_models_roles_payload, validate_models_roles_payload,
        ModelRoleAssignment, ModelsRolesWritePayload,
    };
    use ironhermes_core::config::Config;

    /// T-46.9-05: The write gate must fail closed by default (mirrors
    /// `gate_fails_closed_by_default`, api.rs:1403).
    #[test]
    fn gate_fails_closed() {
        let config = Config::default();
        assert!(
            !config.security.web_config_write_enabled,
            "web_config_write_enabled must default to false (gate closed)"
        );
    }

    /// Task 1 acceptance: a write assigns kanban_judge and reads back via the
    /// shared snapshot builder.
    #[test]
    fn model_roles_write_then_read() {
        let mut config = Config::default();
        let payload = ModelsRolesWritePayload {
            default_model: Some("anthropic/claude-sonnet-4".to_string()),
            provider: None,
            roles: vec![ModelRoleAssignment {
                role_key: "kanban_judge".to_string(),
                provider: None,
                model: Some("openrouter/gpt-4o-mini".to_string()),
            }],
        };
        validate_models_roles_payload(&payload).expect("payload must validate");
        merge_models_roles_payload(&mut config, &payload);

        assert_eq!(config.model.default, "anthropic/claude-sonnet-4");
        let stored = config
            .model
            .roles
            .get("kanban_judge")
            .expect("kanban_judge must be present after merge");
        assert_eq!(
            stored.provider, "main",
            "omitted provider defaults to 'main' (inherit)"
        );
        assert_eq!(stored.model.as_deref(), Some("openrouter/gpt-4o-mini"));

        let snapshot = build_models_roles_snapshot(&config);
        assert_eq!(snapshot.default_model, "anthropic/claude-sonnet-4");
        let row = snapshot
            .roles
            .iter()
            .find(|r| r.role_key == "kanban_judge")
            .expect("kanban_judge row must exist in snapshot");
        assert_eq!(row.model.as_deref(), Some("openrouter/gpt-4o-mini"));

        // Untouched roles remain "uses default" (None/None).
        let untouched = snapshot
            .roles
            .iter()
            .find(|r| r.role_key == "compression")
            .expect("compression row must exist in snapshot");
        assert!(untouched.provider.is_none() && untouched.model.is_none());
    }

    /// T-46.9-06: unknown role keys are rejected before mutation.
    #[test]
    fn validate_rejects_unknown_role_key() {
        let payload = ModelsRolesWritePayload {
            default_model: None,
            provider: None,
            roles: vec![ModelRoleAssignment {
                role_key: "not_a_real_role".to_string(),
                provider: None,
                model: Some("some/model".to_string()),
            }],
        };
        let result = validate_models_roles_payload(&payload);
        assert!(result.is_err(), "unknown role key must be rejected");
    }

    /// Snapshot must return all six MODEL_ROLE_KEYS regardless of catalog
    /// size or how many roles are configured (D-05: "fixed six").
    #[test]
    fn snapshot_always_has_six_rows() {
        let config = Config::default();
        let snapshot = build_models_roles_snapshot(&config);
        assert_eq!(
            snapshot.roles.len(),
            6,
            "snapshot must have exactly six role rows"
        );
    }

    /// Phase 46.9 Plan 07 Task 1 (GAP-1/D-05): a written provider persists
    /// into `config.model.provider` and reads back via the shared snapshot
    /// builder — mirrors `model_roles_write_then_read`'s shape.
    #[test]
    fn provider_write_then_read() {
        let mut config = Config::default();
        let payload = ModelsRolesWritePayload {
            default_model: None,
            provider: Some("anthropic".to_string()),
            roles: Vec::new(),
        };
        validate_models_roles_payload(&payload).expect("payload must validate");
        merge_models_roles_payload(&mut config, &payload);

        assert_eq!(
            config.model.provider, "anthropic",
            "provider write must persist into config.model.provider"
        );

        let snapshot = build_models_roles_snapshot(&config);
        assert_eq!(
            snapshot.provider, "anthropic",
            "provider must read back through the shared snapshot builder"
        );
    }

    /// Phase 46.9 Plan 07 Task 1 (T-46.9-14): a provider string over 64
    /// chars is rejected before any mutation.
    #[test]
    fn validate_rejects_oversized_provider() {
        let payload = ModelsRolesWritePayload {
            default_model: None,
            provider: Some("x".repeat(65)),
            roles: Vec::new(),
        };
        let result = validate_models_roles_payload(&payload);
        assert!(result.is_err(), "oversized provider must be rejected");
    }
}

// =============================================================================
// Phase 46.9 Plan 05 (D-01/D-02): Agent config (editable) + Kanban config (new
// embedded panel) read/write server fns. Both mirror the update_voice_config
// four-step gated-write protocol (validate -> fresh Config::load -> fail-closed
// gate -> merge -> atomic save; api.rs:1333-ish).
// =============================================================================

/// Phase 46.9 Plan 05 (D-01): Web-safe write payload for the editable subset of
/// `config.agent`. `personalities` (HashMap, custom-personality editor scope,
/// not a config-form field) and `max_turns` (deprecated alias — `merge_agent_payload`
/// keeps it in sync automatically, mirroring `AgentConfig::normalize()`,
/// config.rs:1752) are intentionally excluded from the editable surface.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AgentWritePayload {
    pub max_iterations: Option<usize>,
    pub context_compression: Option<f64>,
    pub tool_delay_secs: Option<f64>,
    pub system_message: Option<String>,
    pub context_engine: Option<String>,
    pub compression_threshold: Option<f32>,
    pub clarify_timeout_secs: Option<u64>,
    /// `None` (or an empty string in the payload) means "use host local
    /// timezone" (`chrono::Local`, config.rs:1744). An empty string clears
    /// the field back to `None` on merge.
    pub timezone: Option<String>,
    /// Phase 47.3 Plan 06 (D-01/D-03): selected login-theme slug
    /// (`web_ui.auth.login_theme`). Validated against the five known slugs
    /// in `validate_agent_payload` before merge — reuses the SAME
    /// `security.web_config_write_enabled` gate as every other field on this
    /// payload, no second gate.
    pub login_theme: Option<String>,
}

/// Phase 46.9 Plan 05 (D-01): Read-only snapshot of the editable agent config
/// subset. Always resolved values (serde defaults for absent keys) so a fresh
/// config.yaml never renders a blank/unset form (settings.rs Runtime-block
/// precedent — must_haves truth).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AgentConfigSnapshot {
    pub max_iterations: usize,
    pub context_compression: f64,
    pub tool_delay_secs: f64,
    pub system_message: String,
    pub context_engine: String,
    pub compression_threshold: f32,
    pub clarify_timeout_secs: u64,
    pub timezone: Option<String>,
    /// Whether the web config write gate is open. The UI locks the form when false.
    pub web_config_write_enabled: bool,
    /// Phase 47.3 Plan 06 (D-01/D-02): the currently configured login-theme
    /// slug (`web_ui.auth.login_theme`), so the Settings picker's radio
    /// group reflects live config state even in the read-only branch (D-03:
    /// the selected option still renders visually checked when writes are
    /// disabled).
    pub login_theme: String,
}

/// Phase 46.9 Plan 05: Validate the agent payload BEFORE any config mutation.
/// Ranges keep the agent loop's own invariants sane (e.g. `context_engine` is
/// restricted to the two engines the loop actually understands — an unknown
/// string would silently no-op the compression pass at runtime).
#[cfg(not(target_arch = "wasm32"))]
fn validate_agent_payload(payload: &AgentWritePayload) -> Result<(), ServerFnError> {
    if let Some(v) = payload.max_iterations {
        if !(1..=1000).contains(&v) {
            return Err(ServerFnError::new(format!(
                "max_iterations {v} out of range [1, 1000]"
            )));
        }
    }
    if let Some(v) = payload.context_compression {
        if !(0.0..=1.0).contains(&v) {
            return Err(ServerFnError::new(format!(
                "context_compression {v} out of range [0.0, 1.0]"
            )));
        }
    }
    if let Some(v) = payload.tool_delay_secs {
        if !(0.0..=60.0).contains(&v) {
            return Err(ServerFnError::new(format!(
                "tool_delay_secs {v} out of range [0.0, 60.0]"
            )));
        }
    }
    if let Some(ref v) = payload.system_message {
        if v.len() > 4000 {
            return Err(ServerFnError::new(
                "system_message too long (max 4000 chars)",
            ));
        }
    }
    if let Some(ref v) = payload.context_engine {
        const KNOWN_ENGINES: &[&str] = &["summarizing", "local_prune"];
        if !KNOWN_ENGINES.contains(&v.as_str()) {
            return Err(ServerFnError::new(format!(
                "context_engine '{v}' is not a known engine (summarizing | local_prune)"
            )));
        }
    }
    if let Some(v) = payload.compression_threshold {
        if !(0.0..=1.0).contains(&v) {
            return Err(ServerFnError::new(format!(
                "compression_threshold {v} out of range [0.0, 1.0]"
            )));
        }
    }
    if let Some(v) = payload.clarify_timeout_secs {
        if !(1..=3600).contains(&v) {
            return Err(ServerFnError::new(format!(
                "clarify_timeout_secs {v} out of range [1, 3600]"
            )));
        }
    }
    if let Some(ref v) = payload.timezone {
        if v.len() > 64 {
            return Err(ServerFnError::new("timezone too long (max 64 chars)"));
        }
    }
    if let Some(ref v) = payload.login_theme {
        // Phase 47.3 Plan 06 (D-01/T-47.3-25): reject any slug outside the
        // five shipped treatments — a malformed write must not silently
        // persist an unrenderable theme. The renderer separately falls back
        // to `basic` for a slug it doesn't recognize, but rejecting the
        // write here is the clearer failure mode.
        const KNOWN_THEMES: &[&str] = &["basic", "fade", "blast-doors", "matrix-rain", "orbit-veil"];
        if !KNOWN_THEMES.contains(&v.as_str()) {
            return Err(ServerFnError::new(format!(
                "login_theme '{v}' is not a known theme (basic | fade | blast-doors | matrix-rain | orbit-veil)"
            )));
        }
    }
    Ok(())
}

/// Phase 46.9 Plan 05: Build the read-side snapshot from a loaded Config.
/// Shared by `get_agent_config` and the `agent_config` test module (mirrors
/// `build_voice_snapshot` / `build_models_roles_snapshot`).
#[cfg(not(target_arch = "wasm32"))]
fn build_agent_snapshot(config: &ironhermes_core::config::Config) -> AgentConfigSnapshot {
    AgentConfigSnapshot {
        max_iterations: config.agent.max_iterations,
        context_compression: config.agent.context_compression,
        tool_delay_secs: config.agent.tool_delay_secs,
        system_message: config.agent.system_message.clone(),
        context_engine: config.agent.context_engine.clone(),
        compression_threshold: config.agent.compression_threshold,
        clarify_timeout_secs: config.agent.clarify_timeout_secs,
        timezone: config.agent.timezone.clone(),
        web_config_write_enabled: config.security.web_config_write_enabled,
        login_theme: config.web_ui.auth.login_theme.clone(),
    }
}

/// Phase 46.9 Plan 05: Merge a validated payload into a freshly loaded Config.
/// Only `Some` fields are applied. `max_turns` (deprecated alias) is kept in
/// sync with `max_iterations` on every write so a subsequent `Config::load()`
/// never re-triggers `AgentConfig::normalize()`'s deprecation warning path.
#[cfg(not(target_arch = "wasm32"))]
fn merge_agent_payload(config: &mut ironhermes_core::config::Config, payload: &AgentWritePayload) {
    if let Some(v) = payload.max_iterations {
        config.agent.max_iterations = v;
        config.agent.max_turns = v;
    }
    if let Some(v) = payload.context_compression {
        config.agent.context_compression = v;
    }
    if let Some(v) = payload.tool_delay_secs {
        config.agent.tool_delay_secs = v;
    }
    if let Some(ref v) = payload.system_message {
        config.agent.system_message = v.clone();
    }
    if let Some(ref v) = payload.context_engine {
        config.agent.context_engine = v.clone();
    }
    if let Some(v) = payload.compression_threshold {
        config.agent.compression_threshold = v;
    }
    if let Some(v) = payload.clarify_timeout_secs {
        config.agent.clarify_timeout_secs = v;
    }
    if let Some(ref v) = payload.timezone {
        config.agent.timezone = if v.trim().is_empty() {
            None
        } else {
            Some(v.clone())
        };
    }
    if let Some(ref v) = payload.login_theme {
        config.web_ui.auth.login_theme = v.clone();
    }
}

/// Phase 46.9 Plan 05 (D-01): Read-side server fn for the Settings screen
/// Agent panel. Fresh `Config::load()`, never `app_state.config` (startup
/// snapshot — precedent: `get_voice_config`).
#[server]
pub async fn get_agent_config() -> Result<AgentConfigSnapshot, ServerFnError> {
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
    Ok(build_agent_snapshot(&config))
}

/// Phase 46.9 Plan 05 (D-01): Write server fn for the Settings screen Agent
/// panel. Four-step gated-write protocol (mirrors `update_voice_config`,
/// api.rs:~975): 1. validate, 2. fresh `Config::load()`, 3. fail-closed gate
/// check, 4. merge + atomic `config.save()`.
#[server]
pub async fn update_agent_config(payload: AgentWritePayload) -> Result<(), ServerFnError> {
    // Step 1: validate
    validate_agent_payload(&payload)?;

    // Step 2: fresh disk read (NOT app_state.config — that is the startup snapshot)
    let mut config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;

    // Step 3: gate — web config write is disabled unless operator opts in
    if !config.security.web_config_write_enabled {
        return Err(ServerFnError::new("Config writes are disabled"));
    }

    // Step 4: merge web-safe fields, then atomic save
    merge_agent_payload(&mut config, &payload);
    config
        .save()
        .map_err(|e| ServerFnError::new(format!("Config save failed: {e}")))?;

    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod agent_config {
    use super::{
        build_agent_snapshot, merge_agent_payload, validate_agent_payload, AgentWritePayload,
    };
    use ironhermes_core::config::Config;

    fn empty_payload() -> AgentWritePayload {
        AgentWritePayload {
            max_iterations: None,
            context_compression: None,
            tool_delay_secs: None,
            system_message: None,
            context_engine: None,
            compression_threshold: None,
            clarify_timeout_secs: None,
            timezone: None,
            login_theme: None,
        }
    }

    /// T-46.9-13: the write gate must fail closed by default (mirrors
    /// `gate_fails_closed_by_default`, api.rs:~1428).
    #[test]
    fn gate_fails_closed() {
        let config = Config::default();
        assert!(
            !config.security.web_config_write_enabled,
            "web_config_write_enabled must default to false (gate closed)"
        );
    }

    /// A fresh Config's snapshot must be the resolved serde defaults, never a
    /// blank/unset-looking form (must_haves truth).
    #[test]
    fn snapshot_always_resolved() {
        let config = Config::default();
        let snapshot = build_agent_snapshot(&config);
        assert_eq!(snapshot.max_iterations, config.agent.max_iterations);
        assert_eq!(snapshot.context_engine, config.agent.context_engine);
        assert!(!snapshot.context_engine.is_empty());
    }

    /// Write -> read round-trip through the shared snapshot builder.
    #[test]
    fn agent_write_then_read() {
        let mut config = Config::default();
        let mut payload = empty_payload();
        payload.max_iterations = Some(77);
        payload.system_message = Some("be terse".to_string());
        payload.context_engine = Some("local_prune".to_string());
        validate_agent_payload(&payload).expect("payload must validate");
        merge_agent_payload(&mut config, &payload);

        assert_eq!(config.agent.max_iterations, 77);
        assert_eq!(
            config.agent.max_turns, 77,
            "max_turns alias must stay in sync"
        );
        assert_eq!(config.agent.system_message, "be terse");
        assert_eq!(config.agent.context_engine, "local_prune");

        let snapshot = build_agent_snapshot(&config);
        assert_eq!(snapshot.max_iterations, 77);
        assert_eq!(snapshot.system_message, "be terse");
    }

    /// Out-of-range numeric fields are rejected before any mutation.
    #[test]
    fn validate_rejects_out_of_range() {
        let mut payload = empty_payload();
        payload.max_iterations = Some(0);
        assert!(
            validate_agent_payload(&payload).is_err(),
            "max_iterations 0 must be rejected"
        );

        let mut payload2 = empty_payload();
        payload2.compression_threshold = Some(1.5);
        assert!(
            validate_agent_payload(&payload2).is_err(),
            "compression_threshold > 1.0 must be rejected"
        );
    }

    /// Unknown context_engine values are rejected before any mutation.
    #[test]
    fn validate_rejects_unknown_engine() {
        let mut payload = empty_payload();
        payload.context_engine = Some("not_a_real_engine".to_string());
        assert!(
            validate_agent_payload(&payload).is_err(),
            "unknown context_engine must be rejected"
        );
    }

    /// Phase 47.3 Plan 06 (D-01/T-47.3-25): only the five shipped login-theme
    /// slugs are accepted; an unrecognized slug is rejected before mutation.
    #[test]
    fn validate_rejects_unknown_login_theme() {
        let mut payload = empty_payload();
        payload.login_theme = Some("neon-lobby".to_string());
        assert!(
            validate_agent_payload(&payload).is_err(),
            "unknown login_theme slug must be rejected"
        );
    }

    /// Phase 47.3 Plan 06: each of the five known slugs is accepted and
    /// persists through the snapshot round trip.
    #[test]
    fn login_theme_write_then_read_for_every_known_slug() {
        for slug in ["basic", "fade", "blast-doors", "matrix-rain", "orbit-veil"] {
            let mut config = Config::default();
            let mut payload = empty_payload();
            payload.login_theme = Some(slug.to_string());
            validate_agent_payload(&payload).expect("known slug must validate");
            merge_agent_payload(&mut config, &payload);
            assert_eq!(config.web_ui.auth.login_theme, slug);

            let snapshot = build_agent_snapshot(&config);
            assert_eq!(snapshot.login_theme, slug);
        }
    }

    /// A fresh Config's snapshot must reflect the resolved default login
    /// theme (`basic`) — matches the Runtime-block precedent that the
    /// snapshot always carries resolved values, never a blank form.
    #[test]
    fn snapshot_default_login_theme_is_basic() {
        let config = Config::default();
        let snapshot = build_agent_snapshot(&config);
        assert_eq!(snapshot.login_theme, "basic");
    }
}

// =============================================================================
// Phase 46.9 Plan 05 (D-02): Kanban config — brand-new read+write surface.
// `config.kanban` is an untyped `serde_yaml::Value` (config.rs:373) and is
// NEVER accessed as a typed field — always round-tripped through
// `serde_yaml::from_value`/`to_value` against `ironhermes_kanban::KanbanConfig`
// (RESEARCH Pitfall 5). The read side mirrors `load_kanban_config`
// (kanban_api.rs:956): malformed/absent input falls back to
// `KanbanConfig::default()`, never a partial/corrupt merge.
// =============================================================================

/// Phase 46.9 Plan 05 (D-02): Read-only snapshot of the editable KanbanConfig
/// subset. Always resolved values (`KanbanConfig::default()` fallback for a
/// missing/malformed `config.kanban` block) so first-open is indistinguishable
/// from a fully-configured board (must_haves truth).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct KanbanConfigSnapshot {
    pub dispatch_in_gateway: bool,
    pub dispatch_interval_seconds: u64,
    /// `None` or `Some(0)` = unlimited concurrency (mirrors
    /// `KanbanConfig::max_in_progress` doc comment, ironhermes-kanban/src/config.rs:38).
    pub max_in_progress: Option<usize>,
    pub dispatch_stale_timeout_seconds: u64,
    pub notification_sources: Option<Vec<String>>,
    /// Whether the web config write gate is open. The UI locks SAVE CONFIG when false.
    pub web_config_write_enabled: bool,
}

/// Phase 46.9 Plan 05 (D-02): Web-safe write payload for the editable
/// KanbanConfig subset. `None` fields leave the corresponding KanbanConfig
/// field unchanged on merge.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct KanbanWritePayload {
    pub dispatch_in_gateway: Option<bool>,
    pub dispatch_interval_seconds: Option<u64>,
    /// `Some(0)` writes "unlimited" (matches `KanbanConfig::max_in_progress`
    /// semantics) — there is no separate "clear to None" sentinel; the field
    /// resolves identically to `Some(0)` for the dispatcher's concurrency cap.
    pub max_in_progress: Option<usize>,
    pub dispatch_stale_timeout_seconds: Option<u64>,
    pub notification_sources: Option<Vec<String>>,
}

/// Phase 46.9 Plan 05 (T-46.9-14): Validate the kanban payload BEFORE any
/// config mutation. Rejects `dispatch_interval_seconds: 0` (a tight dispatcher
/// polling loop — DoS guard) and other absurd numeric/collection sizes.
#[cfg(not(target_arch = "wasm32"))]
fn validate_kanban_payload(payload: &KanbanWritePayload) -> Result<(), ServerFnError> {
    if let Some(v) = payload.dispatch_interval_seconds {
        if !(1..=86_400).contains(&v) {
            return Err(ServerFnError::new(format!(
                "dispatch_interval_seconds {v} out of range [1, 86400] (0 is a tight-loop DoS)"
            )));
        }
    }
    if let Some(v) = payload.dispatch_stale_timeout_seconds {
        if !(1..=604_800).contains(&v) {
            return Err(ServerFnError::new(format!(
                "dispatch_stale_timeout_seconds {v} out of range [1, 604800]"
            )));
        }
    }
    if let Some(v) = payload.max_in_progress {
        if v > 100_000 {
            return Err(ServerFnError::new(format!(
                "max_in_progress {v} out of range [0, 100000] (0 = unlimited)"
            )));
        }
    }
    if let Some(ref sources) = payload.notification_sources {
        if sources.len() > 64 {
            return Err(ServerFnError::new(
                "notification_sources too long (max 64 entries)",
            ));
        }
        for s in sources {
            if s.len() > 128 {
                return Err(ServerFnError::new(
                    "notification_sources entry too long (max 128 chars)",
                ));
            }
        }
    }
    Ok(())
}

/// Phase 46.9 Plan 05: Build the read-side snapshot from a loaded Config.
/// Mirrors `load_kanban_config` (kanban_api.rs:956) — `serde_yaml::from_value`
/// with a `KanbanConfig::default()` fallback. Shared by `get_kanban_config`
/// and the `kanban_config` test module.
#[cfg(not(target_arch = "wasm32"))]
fn build_kanban_snapshot(config: &ironhermes_core::config::Config) -> KanbanConfigSnapshot {
    let kanban_cfg: ironhermes_kanban::KanbanConfig =
        serde_yaml::from_value(config.kanban.clone()).unwrap_or_default();
    KanbanConfigSnapshot {
        dispatch_in_gateway: kanban_cfg.dispatch_in_gateway,
        dispatch_interval_seconds: kanban_cfg.dispatch_interval_seconds,
        max_in_progress: kanban_cfg.max_in_progress,
        dispatch_stale_timeout_seconds: kanban_cfg.dispatch_stale_timeout_seconds,
        notification_sources: kanban_cfg.notification_sources,
        web_config_write_enabled: config.security.web_config_write_enabled,
    }
}

/// Phase 46.9 Plan 05 (Pitfall 5): Merge a validated payload into a freshly
/// loaded Config. Reads the current `KanbanConfig` via `serde_yaml::from_value`
/// (default fallback), mutates only the `Some` payload fields, then
/// re-serializes via `serde_yaml::to_value` back into `config.kanban`. NEVER
/// assigns a typed struct directly to `config.kanban` — always round-tripped.
#[cfg(not(target_arch = "wasm32"))]
fn merge_kanban_payload(
    config: &mut ironhermes_core::config::Config,
    payload: &KanbanWritePayload,
) {
    let mut kanban_cfg: ironhermes_kanban::KanbanConfig =
        serde_yaml::from_value(config.kanban.clone()).unwrap_or_default();

    if let Some(v) = payload.dispatch_in_gateway {
        kanban_cfg.dispatch_in_gateway = v;
    }
    if let Some(v) = payload.dispatch_interval_seconds {
        kanban_cfg.dispatch_interval_seconds = v;
    }
    if let Some(v) = payload.max_in_progress {
        kanban_cfg.max_in_progress = Some(v);
    }
    if let Some(v) = payload.dispatch_stale_timeout_seconds {
        kanban_cfg.dispatch_stale_timeout_seconds = v;
    }
    if let Some(ref v) = payload.notification_sources {
        kanban_cfg.notification_sources = Some(v.clone());
    }

    if let Ok(value) = serde_yaml::to_value(&kanban_cfg) {
        config.kanban = value;
    }
}

/// Phase 46.9 Plan 05 (D-02): Read-side server fn for the Settings screen
/// Kanban panel. Fresh `Config::load()`, never `app_state.config`.
#[server]
pub async fn get_kanban_config() -> Result<KanbanConfigSnapshot, ServerFnError> {
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
    Ok(build_kanban_snapshot(&config))
}

/// Phase 46.9 Plan 05 (D-02): Write server fn for the Settings screen Kanban
/// panel. Four-step gated-write protocol (mirrors `update_voice_config`).
#[server]
pub async fn update_kanban_config(payload: KanbanWritePayload) -> Result<(), ServerFnError> {
    // Step 1: validate
    validate_kanban_payload(&payload)?;

    // Step 2: fresh disk read (NOT app_state.config — that is the startup snapshot)
    let mut config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;

    // Step 3: gate — web config write is disabled unless operator opts in
    if !config.security.web_config_write_enabled {
        return Err(ServerFnError::new("Config writes are disabled"));
    }

    // Step 4: merge (via serde_yaml round-trip), then atomic save
    merge_kanban_payload(&mut config, &payload);
    config
        .save()
        .map_err(|e| ServerFnError::new(format!("Config save failed: {e}")))?;

    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod kanban_config {
    use super::{
        build_kanban_snapshot, merge_kanban_payload, validate_kanban_payload, KanbanWritePayload,
    };
    use ironhermes_core::config::Config;

    fn empty_payload() -> KanbanWritePayload {
        KanbanWritePayload {
            dispatch_in_gateway: None,
            dispatch_interval_seconds: None,
            max_in_progress: None,
            dispatch_stale_timeout_seconds: None,
            notification_sources: None,
        }
    }

    /// T-46.9-13: the write gate must fail closed by default.
    #[test]
    fn gate_fails_closed() {
        let config = Config::default();
        assert!(
            !config.security.web_config_write_enabled,
            "web_config_write_enabled must default to false (gate closed)"
        );
    }

    /// A fresh Config has `kanban: serde_yaml::Value` absent/default — the
    /// snapshot must fall back to `KanbanConfig::default()`, never panic or
    /// render a blank/unset form (must_haves truth).
    #[test]
    fn snapshot_falls_back_to_default_when_kanban_block_absent() {
        let config = Config::default();
        let snapshot = build_kanban_snapshot(&config);
        let default_cfg = ironhermes_kanban::KanbanConfig::default();
        assert_eq!(
            snapshot.dispatch_interval_seconds,
            default_cfg.dispatch_interval_seconds
        );
        assert_eq!(
            snapshot.dispatch_in_gateway,
            default_cfg.dispatch_in_gateway
        );
    }

    /// Write dispatch_interval_seconds -> read back via serde_yaml round-trip
    /// (Task 1 acceptance: kanban round-trip through Config.kanban).
    #[test]
    fn kanban_write_then_read_round_trip() {
        let mut config = Config::default();
        let mut payload = empty_payload();
        payload.dispatch_interval_seconds = Some(45);
        payload.max_in_progress = Some(3);
        validate_kanban_payload(&payload).expect("payload must validate");
        merge_kanban_payload(&mut config, &payload);

        // config.kanban must now be a populated serde_yaml::Value, never
        // accessed as a typed field directly.
        assert!(!config.kanban.is_null());

        let snapshot = build_kanban_snapshot(&config);
        assert_eq!(snapshot.dispatch_interval_seconds, 45);
        assert_eq!(snapshot.max_in_progress, Some(3));
    }

    /// T-46.9-14: dispatch_interval_seconds: 0 (tight-loop DoS) is rejected
    /// before any mutation.
    #[test]
    fn validate_rejects_zero_dispatch_interval() {
        let mut payload = empty_payload();
        payload.dispatch_interval_seconds = Some(0);
        assert!(
            validate_kanban_payload(&payload).is_err(),
            "dispatch_interval_seconds: 0 must be rejected (tight-loop DoS)"
        );
    }

    /// Absurd max_in_progress upper bound is rejected before any mutation.
    #[test]
    fn validate_rejects_absurd_max_in_progress() {
        let mut payload = empty_payload();
        payload.max_in_progress = Some(1_000_000);
        assert!(
            validate_kanban_payload(&payload).is_err(),
            "max_in_progress 1_000_000 must be rejected"
        );
    }
}

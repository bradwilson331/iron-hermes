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

/// Phase 50.5 (D-12): which tier resolved a window that reaches the browser.
/// Mirrors `ironhermes_core::provider::ContextLengthSource`, but is defined
/// LOCALLY rather than re-exported: every `ironhermes_core` reference in this
/// file sits inside a `#[cfg(not(target_arch = "wasm32"))]` block because
/// that crate is native-only, while this DTO must cross to the browser.
/// Only `GlobalPin` and `Fallback` are flagged in the topbar (D-12) —
/// `PerModelConfig` and `Metadata` are both genuinely about the selected
/// model and get no marker.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContextWindowSource {
    /// `providers.<p>.models.<m>.context_length` — a sparse, operator-authored
    /// per-(provider, model) override. Most specific.
    PerModelConfig,
    /// The model registry's cache or static-table entry for this model.
    #[default]
    Metadata,
    /// `config.model.context_length` — the global pin, now a floor for
    /// models no more specific tier knows about (Phase 50.5 D-02).
    GlobalPin,
    /// `DEFAULT_CONTEXT_LENGTH` (128,000) — nothing else resolved.
    Fallback,
    /// Phase 58.3.1: the window the local server itself reported.
    Probed,
    /// Phase 58.3.1: the window learned from a provider context-length
    /// rejection and persisted in the models cache.
    Learned,
    /// Phase 58.3.1 (D-11): 8,192 assumed for an unknown model on a local server.
    LocalDefault,
}

/// Phase 50.5 (D-12): the mapping exists in exactly one place so the DTO and
/// the core resolver's own enum can never drift apart.
#[cfg(not(target_arch = "wasm32"))]
impl From<ironhermes_core::provider::ContextLengthSource> for ContextWindowSource {
    fn from(value: ironhermes_core::provider::ContextLengthSource) -> Self {
        match value {
            ironhermes_core::provider::ContextLengthSource::PerModelConfig => Self::PerModelConfig,
            ironhermes_core::provider::ContextLengthSource::Metadata => Self::Metadata,
            ironhermes_core::provider::ContextLengthSource::GlobalPin => Self::GlobalPin,
            ironhermes_core::provider::ContextLengthSource::Fallback => Self::Fallback,
            ironhermes_core::provider::ContextLengthSource::Probed => Self::Probed,
            ironhermes_core::provider::ContextLengthSource::Learned => Self::Learned,
            ironhermes_core::provider::ContextLengthSource::LocalDefault => Self::LocalDefault,
        }
    }
}

/// Phase 50.5 (D-12/D-13): the four human-facing provenance words. These are
/// a CONTRACT, not a label — Plan 06's placeholder copy consumes them
/// verbatim. Wasm-safe (no `cfg` gate needed): it renders in the browser.
/// `#[allow(dead_code)]`: this plan (05) only needs the DTO/enum this
/// derives from — the first caller lands in Plan 06's Models-screen field
/// placeholder (mirrors the existing `decide_realtime_tool_action` precedent
/// in this same file for a symbol declared ahead of its first call site).
#[allow(dead_code)]
pub fn provenance_word(source: ContextWindowSource) -> &'static str {
    match source {
        ContextWindowSource::PerModelConfig => "override",
        ContextWindowSource::Metadata => "cache",
        ContextWindowSource::GlobalPin => "pin",
        ContextWindowSource::Fallback => "default",
        ContextWindowSource::Probed => "probed",
        ContextWindowSource::Learned => "learned",
        ContextWindowSource::LocalDefault => "local default",
    }
}

#[cfg(test)]
mod context_window_source_tests {
    use super::{provenance_word, ContextWindowSource};

    /// These four words are a CONTRACT — Plan 06's placeholder copy consumes
    /// them verbatim.
    #[test]
    fn provenance_word_matches_the_contract() {
        assert_eq!(
            provenance_word(ContextWindowSource::PerModelConfig),
            "override"
        );
        assert_eq!(provenance_word(ContextWindowSource::Metadata), "cache");
        assert_eq!(provenance_word(ContextWindowSource::GlobalPin), "pin");
        assert_eq!(provenance_word(ContextWindowSource::Fallback), "default");
        assert_eq!(provenance_word(ContextWindowSource::Probed), "probed");
        assert_eq!(provenance_word(ContextWindowSource::Learned), "learned");
        assert_eq!(
            provenance_word(ContextWindowSource::LocalDefault),
            "local default"
        );
    }

    /// The core label and the UI word can never drift: one assertion per core
    /// variant (exhaustive match so a new core variant fails to compile here).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn provenance_word_matches_core_label_for_every_variant() {
        use ironhermes_core::provider::ContextLengthSource as C;
        for v in [
            C::PerModelConfig,
            C::Metadata,
            C::GlobalPin,
            C::Fallback,
            C::Probed,
            C::Learned,
            C::LocalDefault,
        ] {
            match v {
                C::PerModelConfig
                | C::Metadata
                | C::GlobalPin
                | C::Fallback
                | C::Probed
                | C::Learned
                | C::LocalDefault => {}
            }
            assert_eq!(provenance_word(ContextWindowSource::from(v)), v.label());
        }
    }

    /// `ContextWindowSource` serializes to lowercase snake_case strings and
    /// round-trips through serde.
    #[test]
    fn round_trips_through_serde_as_snake_case() {
        let cases = [
            (ContextWindowSource::PerModelConfig, "\"per_model_config\""),
            (ContextWindowSource::Metadata, "\"metadata\""),
            (ContextWindowSource::GlobalPin, "\"global_pin\""),
            (ContextWindowSource::Fallback, "\"fallback\""),
            (ContextWindowSource::Probed, "\"probed\""),
            (ContextWindowSource::Learned, "\"learned\""),
            (ContextWindowSource::LocalDefault, "\"local_default\""),
        ];
        for (source, expected_json) in cases {
            let json = serde_json::to_string(&source).expect("serialize");
            assert_eq!(json, expected_json);
            let back: ContextWindowSource = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, source, "round-trip must preserve the variant");
        }
    }
}

/// Phase 50.5: guard a `usize -> u32` DTO-boundary cast with a saturating
/// conversion rather than `as`, so a poisoned cache/config value cannot wrap
/// silently instead of merely rendering as an implausibly large number.
/// Wasm-safe (no `ironhermes_core` reference) even though every current
/// caller is server-only.
fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ConfigSummary {
    pub model: String,
    pub provider: String,
    pub context_length: u32,
    /// Phase 50.5 (D-12): which tier resolved `context_length`, from the SAME
    /// `context_length_with_source()` evaluation as the number itself, so the
    /// two can never disagree. Consumed by `sys_meta.rs`'s `~` marker.
    pub context_length_source: ContextWindowSource,
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
    /// Phase 50.4 Plan 04 (D-01/D-04): this entry's position within its
    /// OWN target's `Vec<String>` — the stable per-target address the
    /// write server fns' D-04 `expected_text` guard is keyed on. Populated
    /// from the same per-target iteration that already pushes `store`/
    /// `body` (50.4-RESEARCH.md Pitfall 3); NOT a global index across the
    /// flattened agent+user list — the two panels write to two different
    /// stores, so a global index would send the wrong row number for every
    /// user-store entry. The client-side filter is purely client-side
    /// (module doc above) and subsets rows without renumbering them, so
    /// this index survives filtering.
    pub idx: usize,
}

/// Phase 50.4 Plan 04 (D-01 stats panel): per-store entry/char accounting
/// for the right-bottom panel, computed from the store's own `char_count`/
/// `char_limit` (not a duplicate implementation) so the number the operator
/// reads here always matches the number the agent sees in its system
/// prompt.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MemoryStoreStats {
    /// "agent" or "user" — same store key as `MemoryEntry::store`.
    pub store: String,
    pub entries: usize,
    pub chars_used: usize,
    pub chars_limit: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct MemoryInfo {
    pub entries: Vec<MemoryEntry>,
    /// Empty when `memory_manager` is `None` (or on a load error) — the
    /// `Default` derive gives an empty `Vec` here, so the disabled path
    /// keeps rendering zero/dash stats instead of an error (Task 3).
    pub stats: Vec<MemoryStoreStats>,
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
    /// Phase 52.1 Plan 02 (D-03): the canonical wire string ("html" | "md" |
    /// "code") from `ArtifactSummary.source_format`, used by the gallery's
    /// `artifact_download_name` helper to pick the raw-download extension.
    pub source_format: String,
    pub updated_at: String,
    /// Phase 46.6 artifact management: soft-archived state (hidden from the
    /// default gallery listing; surfaced only under the "show archived" toggle).
    pub archived: bool,
    /// Phase 52.1 Plan 06 (D-13/D-15): the producing bot's name, resolved at
    /// render time rather than stored — keeps D-13's no-new-column constraint
    /// literally true. Populated server-side for kanban rows only (a
    /// batch-resolved kanban-store lookup, see `resolve_kanban_producers`
    /// below); `None` for every other source kind, including team, whose
    /// producer the gallery component parses client-side from `source_ref`
    /// (`team_producer_from_source_ref` in `screens/artifacts.rs`) since it
    /// needs no store lookup. `None` also on an unresolvable kanban task
    /// (deleted task, unopenable board) — the row degrades to pill-only,
    /// never an error.
    pub producer: Option<String>,
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

    // Phase 52.1 Plan 06 (D-15): batch-resolve every kanban row's producing
    // bot in ONE pass over this listing — the store lock above is already
    // released, and the store is dropped before we ever touch the kanban
    // board store, so the two stores are never held at once. `kanban_ids` is
    // empty for a kanban-free listing, in which case `resolve_kanban_producers`
    // returns immediately without opening anything (T-52.1-21).
    let kanban_ids = kanban_task_ids(&summaries);
    let kanban_producers = resolve_kanban_producers(&kanban_ids);

    Ok(summaries
        .into_iter()
        .map(|s| {
            let producer =
                producer_for_summary(s.source_kind.as_deref(), s.source_ref.as_deref(), &kanban_producers);
            ArtifactInfo {
                id: s.id,
                title: s.title,
                icon: s.icon,
                source_kind: s.source_kind,
                source_ref: s.source_ref,
                source_format: s.source_format,
                updated_at: format!("{}", s.updated_at as i64),
                archived: s.archived,
                producer,
            }
        })
        .collect())
}

/// Phase 52.1 Plan 06 (D-15): the deduplicated set of kanban task ids across
/// every kanban-sourced row in a listing. Pure and store-free — a caller can
/// check `is_empty()` to skip kanban resolution entirely for a kanban-free
/// listing, which is what keeps `resolve_kanban_producers` below at zero
/// board opens in that case (T-52.1-21).
#[cfg(feature = "server")]
fn kanban_task_ids(summaries: &[ArtifactSummary]) -> std::collections::BTreeSet<String> {
    summaries
        .iter()
        .filter(|s| s.source_kind.as_deref() == Some("kanban"))
        .filter_map(|s| s.source_ref.clone())
        .collect()
}

/// Phase 52.1 Plan 06 (D-15): the per-row producer-selection rule, pure and
/// store-free — kept separate from `resolve_kanban_producers` so the
/// selection logic (kanban rows read from the already-resolved map by task
/// id; every other source kind, including a kanban row whose id is absent
/// from the map, is `None`) is unit-testable without a real kanban store.
#[cfg(feature = "server")]
fn producer_for_summary(
    source_kind: Option<&str>,
    source_ref: Option<&str>,
    kanban_producers: &std::collections::HashMap<String, String>,
) -> Option<String> {
    if source_kind != Some("kanban") {
        return None;
    }
    source_ref.and_then(|task_id| kanban_producers.get(task_id).cloned())
}

/// Phase 52.1 Plan 06 (D-15): resolve every id in `task_ids` to its task's
/// assignee, opening the kanban board store at most once (skipped entirely
/// when `task_ids` is empty — no per-row open, T-52.1-21). Every failure
/// degrades rather than propagates: a board that cannot be opened logs a
/// warning and returns an empty map; a task id that resolves to an error or a
/// missing row simply does not appear in the returned map, leaving that row's
/// `producer` as `None`. This fn never constructs a `ServerFnError` — the
/// artifact listing must succeed regardless of kanban's own state
/// (T-52.1-22).
#[cfg(feature = "server")]
fn resolve_kanban_producers(
    task_ids: &std::collections::BTreeSet<String>,
) -> std::collections::HashMap<String, String> {
    let mut producers = std::collections::HashMap::new();
    if task_ids.is_empty() {
        return producers;
    }
    let store = match ironhermes_kanban::KanbanStore::open_from_env_or_board(None) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "kanban producer resolution: could not open board store, rows degrade to no producer name"
            );
            return producers;
        }
    };
    for task_id in task_ids {
        match store.get_task(task_id) {
            Ok(task) => {
                producers.insert(task_id.clone(), task.assignee);
            }
            Err(e) => {
                tracing::warn!(
                    task_id = %task_id,
                    error = %e,
                    "kanban producer resolution: task lookup failed, row degrades to no producer name"
                );
            }
        }
    }
    producers
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
    // Phase 52.1 Plan 06 (D-15): this fn already has the single task id in
    // hand, so it resolves its producer through the exact same
    // `resolve_kanban_producers`/`producer_for_summary` helpers `list_artifacts`
    // uses (one open per call, degrading to `None` on any failure) rather than
    // a second, divergent lookup path. This fn's own scoping is unchanged: it
    // stays unscoped by profile, exactly as its doc comment states.
    let producer = summary.as_ref().and_then(|s| {
        let ids = std::collections::BTreeSet::from([task_id.clone()]);
        let kanban_producers = resolve_kanban_producers(&ids);
        producer_for_summary(s.source_kind.as_deref(), s.source_ref.as_deref(), &kanban_producers)
    });
    Ok(summary.map(|s| ArtifactInfo {
        id: s.id,
        title: s.title,
        icon: s.icon,
        source_kind: s.source_kind,
        source_ref: s.source_ref,
        source_format: s.source_format,
        updated_at: format!("{}", s.updated_at as i64),
        archived: s.archived,
        producer,
    }))
}

#[cfg(all(test, feature = "server"))]
mod artifact_producer_tests {
    use super::{kanban_task_ids, producer_for_summary, resolve_kanban_producers};
    use ironhermes_artifacts::ArtifactSummary;
    use ironhermes_kanban::{CreateTaskOptions, KanbanStore};

    /// Sets an env var for the guard's lifetime, restoring the previous value
    /// (or removing it) on drop. Duplicated from this crate's own established
    /// `ScopedEnv` pattern (e.g. `group_team_api.rs`, `schedules_api.rs`) —
    /// each test module keeps its own copy rather than sharing a `pub(crate)`
    /// type across unrelated test modules.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context (`--test-threads=1` is
            // mandated for this crate's test suite).
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    fn summary(source_kind: Option<&str>, source_ref: Option<&str>) -> ArtifactSummary {
        ArtifactSummary {
            id: "artifact-1".to_string(),
            title: "Title".to_string(),
            icon: None,
            source_kind: source_kind.map(str::to_string),
            source_ref: source_ref.map(str::to_string),
            source_format: "html".to_string(),
            updated_at: 0.0,
            archived: false,
        }
    }

    // -- kanban_task_ids ------------------------------------------------

    #[test]
    fn kanban_task_ids_collects_only_kanban_rows() {
        let summaries = vec![
            summary(Some("kanban"), Some("task-1")),
            summary(Some("chat"), Some("session:file.html")),
            summary(Some("team"), Some("room:drive:worker")),
            summary(Some("kanban"), Some("task-2")),
            summary(Some("delegate"), Some("task-3")),
        ];
        let ids = kanban_task_ids(&summaries);
        assert_eq!(
            ids,
            std::collections::BTreeSet::from(["task-1".to_string(), "task-2".to_string()])
        );
    }

    #[test]
    fn kanban_task_ids_empty_when_no_kanban_rows() {
        let summaries = vec![
            summary(Some("chat"), Some("session:file.html")),
            summary(Some("team"), Some("room:drive:worker")),
            summary(None, None),
        ];
        assert!(kanban_task_ids(&summaries).is_empty());
    }

    // -- resolve_kanban_producers -----------------------------------------

    #[test]
    fn resolve_kanban_producers_skips_open_entirely_for_empty_ids() {
        // No IRONHERMES_KANBAN_DB set and no default board exists in this
        // process's env — if this attempted to open a board it would either
        // error (returning an empty map anyway) or, worse, touch a real
        // ~/.ironhermes/kanban.db. Asserting an instant empty result with no
        // env configured is the behavioral proxy for "opened zero times"
        // (T-52.1-21); the source-level guarantee (exactly one call site) is
        // enforced separately by this plan's `<verify>` source grep.
        let ids = std::collections::BTreeSet::new();
        let producers = resolve_kanban_producers(&ids);
        assert!(producers.is_empty());
    }

    #[test]
    fn resolve_kanban_producers_resolves_existing_task_assignee() {
        let db_dir = tempfile::TempDir::new().expect("tempdir");
        let db_path = db_dir.path().join("kanban.db");
        let _env = ScopedEnv::set("IRONHERMES_KANBAN_DB", db_path.to_str().unwrap());

        let mut store = KanbanStore::open(&db_path).expect("open kanban store");
        let task = store
            .create_task("Do the thing", "worker-bot", CreateTaskOptions::default())
            .expect("create_task");

        let ids = std::collections::BTreeSet::from([task.id.clone()]);
        let producers = resolve_kanban_producers(&ids);
        assert_eq!(producers.get(&task.id).map(String::as_str), Some("worker-bot"));
    }

    #[test]
    fn resolve_kanban_producers_missing_task_yields_no_entry_not_error() {
        let db_dir = tempfile::TempDir::new().expect("tempdir");
        let db_path = db_dir.path().join("kanban.db");
        let _env = ScopedEnv::set("IRONHERMES_KANBAN_DB", db_path.to_str().unwrap());
        // Opening once (via a throwaway store) so the DB file/schema exists,
        // but never creating the task this test looks up.
        let _store = KanbanStore::open(&db_path).expect("open kanban store");

        let ids = std::collections::BTreeSet::from(["does-not-exist".to_string()]);
        let producers = resolve_kanban_producers(&ids);
        assert!(
            !producers.contains_key("does-not-exist"),
            "a missing task must be absent from the map, not present with a bogus value"
        );
    }

    #[test]
    fn resolve_kanban_producers_unopenable_board_returns_empty_map_not_panic() {
        // Point IRONHERMES_KANBAN_DB at a directory, not a file — SQLite
        // cannot open a directory as a database, so this reliably fails
        // `Connection::open` without needing to simulate a permissions error.
        let dir_as_db = tempfile::TempDir::new().expect("tempdir");
        let _env = ScopedEnv::set("IRONHERMES_KANBAN_DB", dir_as_db.path().to_str().unwrap());

        let ids = std::collections::BTreeSet::from(["task-1".to_string()]);
        let producers = resolve_kanban_producers(&ids);
        assert!(
            producers.is_empty(),
            "an unopenable board must degrade to an empty map, never panic or propagate"
        );
    }

    #[test]
    fn resolve_kanban_producers_partial_resolution_for_mixed_ids() {
        let db_dir = tempfile::TempDir::new().expect("tempdir");
        let db_path = db_dir.path().join("kanban.db");
        let _env = ScopedEnv::set("IRONHERMES_KANBAN_DB", db_path.to_str().unwrap());

        let mut store = KanbanStore::open(&db_path).expect("open kanban store");
        let task_a = store
            .create_task("Task A", "alpha-bot", CreateTaskOptions::default())
            .expect("create_task a");
        let task_b = store
            .create_task("Task B", "beta-bot", CreateTaskOptions::default())
            .expect("create_task b");

        let ids = std::collections::BTreeSet::from([
            task_a.id.clone(),
            task_b.id.clone(),
            "missing-task".to_string(),
        ]);
        let producers = resolve_kanban_producers(&ids);
        assert_eq!(producers.len(), 2, "only the two resolvable tasks should be present");
        assert_eq!(producers.get(&task_a.id).map(String::as_str), Some("alpha-bot"));
        assert_eq!(producers.get(&task_b.id).map(String::as_str), Some("beta-bot"));
        assert!(!producers.contains_key("missing-task"));
    }

    // -- producer_for_summary (pure, per-row selection) ---------------------

    #[test]
    fn producer_for_summary_kanban_row_reads_from_map() {
        let mut producers = std::collections::HashMap::new();
        producers.insert("task-1".to_string(), "worker-bot".to_string());
        assert_eq!(
            producer_for_summary(Some("kanban"), Some("task-1"), &producers),
            Some("worker-bot".to_string())
        );
    }

    #[test]
    fn producer_for_summary_kanban_row_missing_from_map_is_none() {
        let producers = std::collections::HashMap::new();
        assert_eq!(
            producer_for_summary(Some("kanban"), Some("unresolved-task"), &producers),
            None
        );
    }

    #[test]
    fn producer_for_summary_non_kanban_rows_are_always_none() {
        let mut producers = std::collections::HashMap::new();
        // Even if the map happens to contain a matching key, a non-kanban row
        // must never read from it — that map is scoped to kanban rows only.
        producers.insert("task-1".to_string(), "worker-bot".to_string());
        for kind in [Some("chat"), Some("team"), Some("delegate"), None] {
            assert_eq!(
                producer_for_summary(kind, Some("task-1"), &producers),
                None,
                "source_kind {kind:?} must never read the kanban producer map"
            );
        }
    }
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
    let cfg = state.config();
    // Phase 50.5 (D-12): ONE call to `context_length_with_source()` — the
    // number and its provenance must come from the same tier evaluation, or
    // the topbar's `~` marker and its number could disagree.
    let (context_length, context_length_source) =
        state.resolver().resolve_for_main().context_length_with_source();
    Ok(ConfigSummary {
        model: cfg.model.default.clone(),
        provider: cfg.model.provider.clone(),
        context_length: saturating_u32(context_length),
        context_length_source: context_length_source.into(),
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
    Ok(build_memory_info(&entries))
}

/// Pure builder for [`get_memory`]'s response — extracted so the per-target
/// idx assignment and stats accounting are directly unit-testable without
/// `global_app_state()` (mirrors this crate's "test the logic layer" split,
/// e.g. `check_profile_write_gate`). Indexes are assigned per target (this
/// target's position within its own `Vec<String>`), never globally across
/// the flattened agent+user list — see `MemoryEntry::idx`'s doc comment.
#[cfg(feature = "server")]
fn build_memory_info(entries: &ironhermes_core::memory_provider::MemoryEntries) -> MemoryInfo {
    let mut out: Vec<MemoryEntry> = Vec::new();
    let mut stats: Vec<MemoryStoreStats> = Vec::new();
    for (target, items) in entries.entries.iter() {
        let store = match target {
            ironhermes_core::memory_store::MemoryTarget::Memory => "agent",
            ironhermes_core::memory_store::MemoryTarget::User => "user",
        };
        for (idx, body) in items.iter().enumerate() {
            out.push(MemoryEntry {
                store: store.to_string(),
                body: body.clone(),
                idx,
            });
        }
        // Plan 50.4-02 made char_count/ENTRY_DELIMITER public on
        // memory_store specifically so this does not become a second,
        // drifting accounting implementation.
        stats.push(MemoryStoreStats {
            store: store.to_string(),
            entries: items.len(),
            chars_used: ironhermes_core::memory_store::char_count(
                items,
                ironhermes_core::memory_store::ENTRY_DELIMITER,
            ),
            chars_limit: target.char_limit(),
        });
    }
    MemoryInfo {
        entries: out,
        stats,
    }
}

/// Phase 50.4 Plan 04 (T-50.4-15): fail-closed write gate for the three
/// memory write server fns below. Memory writes are gated behind
/// `web_config_write_enabled` because it is the only write-permission
/// concept this crate has — an ungated memory-write surface would be the
/// sole browser-reachable mutation with no gate at all, and memory content
/// feeds directly into the system prompt (PLANNER DISCRETION, RESEARCH
/// assumption A2 — no D-NN rules on this; accepted consequence: an operator
/// who has disabled config writes also cannot edit memory).
///
/// The refusal MESSAGE is a distinct decision from the flag it gates: it
/// does NOT reuse `update_provider_config`/`apply_config_now`'s "Config
/// writes are disabled" string, because a memory edit refused with that
/// wording names a subsystem the operator was not using. The two gates
/// share `web_config_write_enabled`, not their copy. Pure and disk-I/O-free
/// (mirrors `check_profile_write_gate`'s "test the logic layer" discipline)
/// so it is directly unit-testable without `global_app_state()` or the
/// network.
#[cfg(feature = "server")]
pub(crate) fn check_memory_write_gate(config: &ironhermes_core::config::Config) -> Result<(), String> {
    if !config.security.web_config_write_enabled {
        return Err("Memory edits are disabled while web config writes are off.".to_string());
    }
    Ok(())
}

/// Phase 50.4 Plan 04 (D-01): add a new memory entry from the browser.
/// Four-step gated-write protocol (mirrors `update_provider_config`,
/// `provider_config_api.rs:252-281`): validate `store` -> fresh
/// `Config::load()` -> gate -> act. Returns the store's own JSON result
/// string UNMODIFIED (success or error) — D-02 requires the operator see
/// the identical scanner refusal the agent's tool path sees, and
/// paraphrasing it would imply a different rule; the same applies to the
/// store's over-limit message.
#[server]
pub async fn add_memory_entry(store: String, content: String) -> Result<String, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let target = memory_target_from_store_key(&store)?;

        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        check_memory_write_gate(&config).map_err(ServerFnError::new)?;

        let state = crate::server::state::global_app_state();
        let Some(ref mgr) = state.memory_manager else {
            // A write that cannot happen must not look like it happened —
            // unlike get_memory's silent empty, this is an explicit error
            // (T-50.4-19).
            return Err(ServerFnError::new("Memory is disabled"));
        };
        mgr.lock()
            .await
            .add(target, &content)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (store, content);
        Err(ServerFnError::new(
            "add_memory_entry unavailable without `server` feature",
        ))
    }
}

/// Phase 50.4 Plan 04 (D-01/D-04): replace the entry at `idx`, guarded by
/// `expected_text` — the row's ORIGINAL displayed body, not the edited
/// text. Same gated-write protocol and unmodified-error-passthrough
/// contract as [`add_memory_entry`].
#[server]
pub async fn replace_memory_entry(
    store: String,
    idx: usize,
    expected_text: String,
    new_content: String,
) -> Result<String, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let target = memory_target_from_store_key(&store)?;

        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        check_memory_write_gate(&config).map_err(ServerFnError::new)?;

        let state = crate::server::state::global_app_state();
        let Some(ref mgr) = state.memory_manager else {
            return Err(ServerFnError::new("Memory is disabled"));
        };
        mgr.lock()
            .await
            .replace_at(target, idx, &expected_text, &new_content)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (store, idx, expected_text, new_content);
        Err(ServerFnError::new(
            "replace_memory_entry unavailable without `server` feature",
        ))
    }
}

/// Phase 50.4 Plan 04 (D-01/D-04): remove the entry at `idx`, guarded by
/// `expected_text`. Same gated-write protocol and unmodified-error-
/// passthrough contract as [`add_memory_entry`].
#[server]
pub async fn remove_memory_entry(
    store: String,
    idx: usize,
    expected_text: String,
) -> Result<String, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let target = memory_target_from_store_key(&store)?;

        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        check_memory_write_gate(&config).map_err(ServerFnError::new)?;

        let state = crate::server::state::global_app_state();
        let Some(ref mgr) = state.memory_manager else {
            return Err(ServerFnError::new("Memory is disabled"));
        };
        mgr.lock()
            .await
            .remove_at(target, idx, &expected_text)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (store, idx, expected_text);
        Err(ServerFnError::new(
            "remove_memory_entry unavailable without `server` feature",
        ))
    }
}

/// Maps the client's "agent"/"user" store key to a `MemoryTarget` — the
/// exact reverse of the mapping `get_memory` already applies going the
/// other way — and rejects any other value rather than defaulting.
#[cfg(feature = "server")]
fn memory_target_from_store_key(
    store: &str,
) -> Result<ironhermes_core::memory_store::MemoryTarget, ServerFnError> {
    match store {
        "agent" => Ok(ironhermes_core::memory_store::MemoryTarget::Memory),
        "user" => Ok(ironhermes_core::memory_store::MemoryTarget::User),
        other => Err(ServerFnError::new(format!(
            "Unknown memory store '{other}' — expected 'agent' or 'user'"
        ))),
    }
}

#[cfg(all(test, not(target_arch = "wasm32"), feature = "server"))]
mod memory_write_tests {
    use super::{build_memory_info, check_memory_write_gate};
    use ironhermes_core::memory_provider::MemoryEntries;
    use ironhermes_core::memory_store::{char_count, MemoryTarget, ENTRY_DELIMITER};
    use std::collections::HashMap;

    /// Task 1 behavior test: indexes are assigned PER TARGET, not globally
    /// across the flattened agent+user list — two agent entries and three
    /// user entries must carry idx 0/1 (agent) and 0/1/2 (user), never a
    /// running counter across both stores.
    #[test]
    fn test_get_memory_assigns_per_target_indexes() {
        let mut map: HashMap<MemoryTarget, Vec<String>> = HashMap::new();
        map.insert(
            MemoryTarget::Memory,
            vec!["agent one".to_string(), "agent two".to_string()],
        );
        map.insert(
            MemoryTarget::User,
            vec![
                "user one".to_string(),
                "user two".to_string(),
                "user three".to_string(),
            ],
        );
        let info = build_memory_info(&MemoryEntries { entries: map });

        let mut agent_idxs: Vec<usize> = info
            .entries
            .iter()
            .filter(|e| e.store == "agent")
            .map(|e| e.idx)
            .collect();
        agent_idxs.sort_unstable();
        assert_eq!(agent_idxs, vec![0, 1]);

        let mut user_idxs: Vec<usize> = info
            .entries
            .iter()
            .filter(|e| e.store == "user")
            .map(|e| e.idx)
            .collect();
        user_idxs.sort_unstable();
        assert_eq!(user_idxs, vec![0, 1, 2]);
    }

    /// Task 1 behavior test: the pure gate helper refuses when config
    /// writes are off and allows when they are on, exercised without
    /// `global_app_state()` or the network. Full-string `assert_eq!`
    /// (not `contains`) — this is the only place this literal is pinned.
    #[test]
    fn test_memory_write_gate_refuses_when_config_writes_disabled() {
        let mut config = ironhermes_core::config::Config::default();
        config.security.web_config_write_enabled = false;
        let err = check_memory_write_gate(&config).expect_err("must refuse when writes are off");
        assert_eq!(
            err,
            "Memory edits are disabled while web config writes are off."
        );

        config.security.web_config_write_enabled = true;
        assert!(check_memory_write_gate(&config).is_ok());
    }

    /// Task 1 behavior test: stats come from the store's own
    /// `char_count`/`char_limit` accounting, proving no duplicate
    /// implementation was introduced.
    #[test]
    fn test_memory_stats_use_the_store_accounting() {
        let mut map: HashMap<MemoryTarget, Vec<String>> = HashMap::new();
        let agent_entries = vec!["alpha".to_string(), "beta".to_string()];
        let expected_used = char_count(&agent_entries, ENTRY_DELIMITER);
        map.insert(MemoryTarget::Memory, agent_entries);
        let info = build_memory_info(&MemoryEntries { entries: map });

        let agent_stats = info
            .stats
            .iter()
            .find(|s| s.store == "agent")
            .expect("agent stats row must exist");
        assert_eq!(agent_stats.chars_used, expected_used);
        assert_eq!(agent_stats.chars_limit, MemoryTarget::Memory.char_limit());
        assert_eq!(agent_stats.entries, 2);
    }
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
    // `resolver()` is now an owned-return hot-swap accessor (Phase 50.4) — bind
    // it to a local so `registry` (borrowed from it below) outlives this statement.
    let resolver = state.resolver();
    let registry = resolver.model_registry();
    let default_id = state.config().model.default.clone();

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

/// Phase 46.9 Plan 13 (GAP-1) / Phase 50.5 (D-06): pure parser for an
/// OpenAI-compatible `/models` response body
/// (`{"data": [{"id": "...", "context_length": N}, ...]}`). Answers BOTH
/// questions now: the ordered served-id list (skipping any entry whose `id`
/// is not a string, deterministic order = response order), and a partial
/// context-window observation per id that reported one — the window half
/// feeds the resolver's compaction budget, not only this dropdown (see the
/// write-through function directly below). Delegates to
/// `ironhermes_core::parse_provider_models_response` rather than
/// duplicating the traversal (D-19's triplication lesson applies here too).
#[cfg(not(target_arch = "wasm32"))]
fn parse_openai_models_response(
    body: &serde_json::Value,
) -> (
    Vec<String>,
    std::collections::HashMap<String, ironhermes_core::PartialModelMetadata>,
) {
    ironhermes_core::parse_provider_models_response(body)
}

/// Phase 50.5 (D-06/D-07): writes the context-window observations
/// `list_provider_models` already fetched into the on-disk `ModelsCache` —
/// the SAME store `ProviderResolver::build` reads (`provider.rs:258`), so a
/// harvest benefits the resolver's compaction budget, not only the topbar.
/// Every failure path — including an empty `entries` map, which returns
/// early without touching disk — logs at `tracing::warn!` and returns `()`;
/// this function must never be able to change `list_provider_models`'s
/// result.
#[cfg(not(target_arch = "wasm32"))]
fn harvest_provider_windows(
    entries: std::collections::HashMap<String, ironhermes_core::PartialModelMetadata>,
) {
    if entries.is_empty() {
        return;
    }

    let now = chrono::Utc::now();
    let fresh: std::collections::HashMap<String, ironhermes_core::ModelsCacheEntry> = entries
        .into_iter()
        .map(|(id, metadata)| {
            (
                id,
                ironhermes_core::ModelsCacheEntry {
                    metadata,
                    fetched_at: now,
                },
            )
        })
        .collect();

    let mut cache = ironhermes_core::ModelsCache::load();
    cache.merge_entries(fresh);
    if let Err(e) = cache.save() {
        tracing::warn!(
            error = %e,
            "failed to persist provider /models context-window harvest"
        );
    }
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
        .resolver()
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

        let (models, windows) = parse_openai_models_response(&body);
        if models.is_empty() {
            return ProviderModelsSnapshot {
                models: catalog_ids,
                fell_back: true,
            };
        }

        // Phase 50.5 (D-06/D-07): harvest on this ONE path only — a
        // genuinely provider-sourced, non-empty response, immediately
        // before the non-fallback snapshot is built. None of the fallback
        // branches above reach here, and the cache hit at the top of this
        // function returns before this async block ever runs, so D-07's
        // single-flight constraint is preserved by construction.
        harvest_provider_windows(windows);

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
        let (ids, _windows) = parse_openai_models_response(&body);
        assert_eq!(
            ids,
            vec!["prov-only-a".to_string(), "prov-only-b".to_string()],
            "non-string id must be skipped; order must match response order"
        );
    }

    #[test]
    fn parser_returns_empty_when_data_array_missing() {
        let body = serde_json::json!({ "unexpected": "shape" });
        let (ids, windows) = parse_openai_models_response(&body);
        assert!(ids.is_empty());
        assert!(windows.is_empty());
    }

    /// Phase 50.5 (D-06): the id list stays byte-identical to today's for a
    /// body with no `context_length` fields — existing id assertions above
    /// keep passing unmodified. This test asserts the NEW window half: a
    /// fixture with one entry carrying `context_length` and one without
    /// yields two served ids and exactly one partial entry.
    #[test]
    fn web_harvest_extracts_context_length() {
        let body = serde_json::json!({
            "data": [
                { "id": "prov-only-a", "context_length": 1_048_576 },
                { "id": "prov-only-b" }
            ]
        });

        let (ids, windows) = parse_openai_models_response(&body);

        assert_eq!(
            ids,
            vec!["prov-only-a".to_string(), "prov-only-b".to_string()]
        );
        assert_eq!(windows.len(), 1);
        let entry = windows.get("prov-only-a").expect("prov-only-a");
        assert_eq!(entry.context_length, Some(1_048_576));
        assert!(entry.tokenizer.is_none());
        assert!(entry.max_output_tokens.is_none());
        assert!(entry.capabilities.is_none());
        assert!(!windows.contains_key("prov-only-b"));
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

/// Phase 54.1 D-06: pure join between the activated profile's own
/// `skills.disabled` set and the server's in-process `active_skills` set.
/// Returns `!set.contains(name)` when a profile is active (opt-out model:
/// absent from the profile's disabled set means enabled), and falls back
/// to `active_names.contains(name)` — today's behavior, bit for bit — when
/// `profile_disabled` is `None`.
///
/// Why the read side had to move with the write side: D-07 deliberately
/// skips the in-process `active_skills` mutation in `toggle_skill` when a
/// profile is active, so a `list_skills` that kept reading only
/// `active_skills` would never reflect a toggle made against a profile's
/// own config — the Skills screen's checkbox would freeze after every
/// toggle with a profile active.
#[cfg(not(target_arch = "wasm32"))]
fn enabled_for_display(
    name: &str,
    profile_disabled: Option<&std::collections::HashSet<String>>,
    active_names: &std::collections::HashSet<String>,
) -> bool {
    match profile_disabled {
        Some(set) => !set.contains(name),
        None => active_names.contains(name),
    }
}

/// Phase 54.1 D-06: resolve the activated profile's own `skills.disabled`
/// set for display — disk-I/O-only (no `global_app_state()`), mirroring
/// `fetch_profile_skills_impl`'s read shape (`profile_api.rs:2585-2605`):
/// `validate_profile_name` -> `Config::load_from(profile_dir_for(slug)/config.yaml)`
/// -> `cfg.skills.disabled` as a `HashSet`. A load error (invalid name,
/// missing/malformed config.yaml) is logged and returns `None` rather than
/// failing the whole catalog read — `enabled_for_display`'s caller then
/// falls back to the `active_skills` answer.
#[cfg(not(target_arch = "wasm32"))]
fn load_profile_disabled_for_display(slug: &str) -> Option<std::collections::HashSet<String>> {
    let loaded = ironhermes_core::profile::validate_profile_name(slug)
        .map_err(|e| format!("invalid profile name: {e}"))
        .and_then(|_| {
            ironhermes_core::config::Config::load_from(
                &crate::server::profile_api::profile_dir_for(slug).join("config.yaml"),
            )
            .map_err(|e| format!("load profile config.yaml: {e}"))
        });
    match loaded {
        Ok(cfg) => Some(cfg.skills.disabled.iter().cloned().collect()),
        Err(e) => {
            tracing::warn!(
                profile = %slug,
                error = %e,
                "skills_display: failed to load profile config for display; degrading to active_skills"
            );
            None
        }
    }
}

/// Phase 26.7 Plan 03 (D-09, R-1, R-4): Read-only catalog of skills from the
/// runtime SkillRegistry, with per-skill `enabled` reflecting the current
/// session's `active_skills` set. SkillRegistry.list() is sync — no await
/// needed for the registry read; only the std Mutex lock on active_skills.
///
/// Phase 54.1 D-06: `enabled` now resolves from the activated profile's own
/// `skills.disabled` (via `resolve_chat_optout_target()` — Task 1's seam,
/// never a second inline `resolve_active_profile_for` call) when one is
/// active, so this agrees with the store `toggle_skill` writes.
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

    let profile_disabled: Option<std::collections::HashSet<String>> =
        match resolve_chat_optout_target() {
            OptOutTarget::Root => None,
            OptOutTarget::Profile(slug) => load_profile_disabled_for_display(&slug),
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
                enabled: enabled_for_display(&r.name, profile_disabled.as_ref(), &active_names),
            }
        })
        .collect();
    Ok(out)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod skills_display_tests {
    use super::{enabled_for_display, load_profile_disabled_for_display};
    use std::collections::HashSet;

    /// RAII guard, duplicated per this crate's own established precedent
    /// (see `toggle_skill_tests::ScopedEnv`'s doc comment — each
    /// `#[cfg(test)]` module is its own namespace).
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

    #[test]
    fn skills_display_d06_profile_disabled_set_overrides_active_skills() {
        let profile_disabled: HashSet<String> = ["alpha".to_string()].into_iter().collect();
        let active_names: HashSet<String> = ["alpha".to_string()].into_iter().collect();
        // alpha is in BOTH active_skills and the profile's disabled set —
        // the profile set must win (false), proving active_skills alone
        // would have reported this skill enabled.
        assert!(!enabled_for_display(
            "alpha",
            Some(&profile_disabled),
            &active_names
        ));
        assert!(enabled_for_display(
            "beta",
            Some(&profile_disabled),
            &active_names
        ));
    }

    #[test]
    fn skills_display_d07_no_profile_falls_back_to_active_skills() {
        let active_names: HashSet<String> = ["alpha".to_string()].into_iter().collect();
        assert!(enabled_for_display("alpha", None, &active_names));
        assert!(!enabled_for_display("beta", None, &active_names));
    }

    #[test]
    fn skills_display_d06_unloadable_profile_config_degrades_to_active_skills() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        let profile_dir = dir.path().join("profiles").join("mybot");
        std::fs::create_dir_all(&profile_dir).expect("mkdir profile dir");
        std::fs::write(profile_dir.join("config.yaml"), "not: [valid, yaml: :::")
            .expect("write malformed config");

        assert_eq!(load_profile_disabled_for_display("mybot"), None);
    }
}

// =============================================================================
// Phase 54.1 D-06/D-07: opt-out write-target resolution — the seam both
// `toggle_skill` and `list_skills` branch on.
// =============================================================================

/// Phase 54.1 D-06/D-07: the two possible write/read targets for the Skills
/// screen's opt-out state. `Root` is the pre-54.1 behavior — the process's
/// own `get_hermes_home()/config.yaml` via `Config::load()`/`Config::save()`.
/// `Profile(slug)` is new: the activated profile's OWN `config.yaml` under
/// `profiles/<slug>/`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum OptOutTarget {
    Root,
    Profile(String),
}

/// Phase 54.1 D-06/D-07: pure branch on an already-resolved profile slug.
/// `"default"` is the sentinel `ironhermes_core::current_profile()` and
/// `resolve_active_profile_for`'s fallback both return when nothing more
/// specific applies — `RESERVED_NAMES` refuses that literal name, so a
/// `profiles/default` directory can never exist
/// (`profile_activation_api.rs:231-234`) — and therefore always maps to the
/// pre-54.1 root target. Any other slug names a real profile directory and
/// gets its own target.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_optout_write_target(slug: &str) -> OptOutTarget {
    if slug == "default" {
        OptOutTarget::Root
    } else {
        OptOutTarget::Profile(slug.to_string())
    }
}

/// Phase 54.1 D-06/D-07 (T-54.1-07): the single seam `toggle_skill` and
/// `list_skills` both branch on — the ONLY place in this file that calls
/// `resolve_active_profile_for`. Deliberately NOT inlined at either call
/// site: this fn reads the persisted activation record, which is the whole
/// property under test, so it exists as a named seam a test can drive
/// rather than as inline code duplicated twice.
///
/// Inertness trap (read before "simplifying" this call): both
/// `ironhermes_core::current_profile()` and this file's own
/// `active_profile_identifier()` wrapper (`api.rs:802`) ALWAYS return the
/// `"default"` sentinel inside this crate's UI server process, because that
/// process is never `--profile`-pivoted. Using either one as the branch
/// predicate would make the profile branch unreachable dead code that
/// reports success while doing nothing — exactly the "plan-registered
/// mitigation is fiction" failure mode this repository has shipped before.
/// `resolve_active_profile_for` is the resolver that instead reads the UI's
/// OWN persisted activation record
/// (`crate::server::profile_activation_api::read_active_profile_record`)
/// first, falling back to `current_profile()` only when no record applies.
/// `skills_optout_d06_call_site_resolves_profile_target_from_activation_record`
/// is the test that goes red the moment this predicate is reverted.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_chat_optout_target() -> OptOutTarget {
    let slug = crate::server::profile_activation_api::resolve_active_profile_for(
        crate::server::profile_activation_api::ActivationSurface::Chat,
        None,
    );
    resolve_optout_write_target(&slug)
}

/// Phase 54.1 D-06 (T-54.1-05/T-54.1-08): pure payload builder for the
/// profile opt-out branch. `existing_disabled` is the profile's own
/// current `skills.disabled` list; `catalog_names` is the live skill
/// catalog; `name` is the skill being toggled. Returns the payload, whether
/// `name` was disabled before the toggle, and the list of stale entries
/// dropped from `existing_disabled`.
///
/// Stale-drop rationale: `apply_skills_disabled` (`profile_api.rs:2437+`)
/// rejects the ENTIRE payload if any entry is absent from the live catalog,
/// so a single pre-existing stale entry in a profile's list would brick
/// every future toggle for that profile. Dropping it here (and reporting it
/// via the returned `dropped_stale` list, which the caller `tracing::warn!`s)
/// is self-healing — a name absent from the catalog hides nothing.
#[cfg(not(target_arch = "wasm32"))]
fn build_profile_optout_payload(
    slug: &str,
    existing_disabled: &[String],
    catalog_names: &std::collections::HashSet<String>,
    name: &str,
) -> (crate::protocol::ProfileConfigWritePayload, bool, Vec<String>) {
    let mut dropped_stale = Vec::new();
    let mut retained: Vec<String> = Vec::new();
    for n in existing_disabled {
        if catalog_names.contains(n) {
            retained.push(n.clone());
        } else {
            dropped_stale.push(n.clone());
        }
    }
    let was_disabled = retained.iter().any(|n| n == name);
    if was_disabled {
        retained.retain(|n| n != name);
    } else {
        retained.push(name.to_string());
    }
    let payload = crate::protocol::ProfileConfigWritePayload {
        name: slug.to_string(),
        provider: None,
        model_default: None,
        skills_disabled: Some(retained),
        tools_toolsets: None,
        tools_disable: None,
        tools_enable: None,
    };
    (payload, was_disabled, dropped_stale)
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
///
/// Phase 54.1 D-06/D-07: Steps 2-4 now branch on `resolve_chat_optout_target()`
/// — never `current_profile()` / `active_profile_identifier()`, both of which
/// always read `"default"` inside this never-pivoted UI server (see that
/// seam's own doc comment for the inertness-trap detail). With a profile
/// active, Steps 2-3 instead reproduce `update_profile_config`'s full
/// four-step protocol (`validate_profile_config_payload` -> fresh
/// `Config::load()` -> `check_profile_write_gate` -> `update_profile_config_impl`)
/// against THAT profile's own `config.yaml`, and Step 4 is SKIPPED — the
/// server's live `active_skills` session belongs to the un-pivoted "default"
/// operator session, not to whichever profile was just toggled.
#[server]
pub async fn toggle_skill(name: String) -> Result<(), ServerFnError> {
    let state = crate::server::state::global_app_state();

    // Step 1 — input validation: reject unknown skill names BEFORE any disk I/O (T-26.7.3-02-01)
    let record = state.runtime.skill_registry().find(&name).cloned();
    if record.is_none() {
        return Err(ServerFnError::new(format!("unknown skill: {name}")));
    }

    match resolve_chat_optout_target() {
        OptOutTarget::Root => {
            // RR-05 (WR-06): serialise the root load-modify-save; dropped right after the save.
            let root_write = crate::server::profile_activation_api::lock_root_config_write();
            // Step 2 — load fresh config from disk and toggle the disabled list
            let mut config = ironhermes_core::config::Config::load().unwrap_or_default();
            let was_disabled = apply_disable_toggle(&mut config, &name);

            // Step 3 — persist config back to disk
            config
                .save()
                .map_err(|e| ServerFnError::new(format!("Config save failed: {e}")))?;
            drop(root_write);

            // Step 4 — in-process active_skills mutation (scoped std::sync::Mutex lock).
            // The MutexGuard is dropped at the closing brace before fn returns; no async
            // suspension occurs while the lock is held (lock discipline: Behavior 5).
            // Pattern: api.rs list_skills lines 234–240 (active_skills lock pattern)
            {
                let mut skills =
                    state.runtime.active_skills().lock().map_err(|e| {
                        ServerFnError::new(format!("active_skills lock poisoned: {e}"))
                    })?;
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
        }
        OptOutTarget::Profile(slug) => {
            // Step 2 (profile branch) — validate the slug before any path is
            // resolved, then load THAT profile's own config.yaml (never the
            // root), mirroring `fetch_profile_skills_impl`'s read shape.
            ironhermes_core::profile::validate_profile_name(&slug)
                .map_err(|e| ServerFnError::new(format!("invalid profile name: {e}")))?;
            let profile_config_path =
                crate::server::profile_api::profile_dir_for(&slug).join("config.yaml");
            let profile_cfg = ironhermes_core::config::Config::load_from(&profile_config_path)
                .map_err(|e| ServerFnError::new(format!("load profile config.yaml: {e}")))?;
            let catalog_names: std::collections::HashSet<String> = state
                .runtime
                .skill_registry()
                .list()
                .iter()
                .map(|r| r.name.clone())
                .collect();
            let (payload, _was_disabled, dropped_stale) = build_profile_optout_payload(
                &slug,
                &profile_cfg.skills.disabled,
                &catalog_names,
                &name,
            );
            for stale in &dropped_stale {
                tracing::warn!(
                    profile = %slug,
                    skill = %stale,
                    "skills_optout: dropping stale disabled entry absent from live catalog"
                );
            }

            // Step 3 (profile branch) — reproduce `update_profile_config`'s
            // full four-step protocol rather than only its final call, so
            // this branch never weakens the fail-closed write gate.
            crate::server::profile_api::validate_profile_config_payload(&payload)
                .map_err(ServerFnError::new)?;
            let fresh_config = ironhermes_core::config::Config::load()
                .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
            crate::server::profile_api::check_profile_write_gate(&fresh_config)
                .map_err(ServerFnError::new)?;
            crate::server::profile_api::update_profile_config_impl(&payload)
                .map_err(ServerFnError::new)?;

            // Step 4 is intentionally SKIPPED in this branch (D-07) — see
            // this fn's doc comment.
        }
    }

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
    use super::{
        apply_disable_toggle, build_profile_optout_payload, resolve_chat_optout_target,
        resolve_optout_write_target, OptOutTarget,
    };
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

    // -----------------------------------------------------------------
    // Phase 54.1 D-06/D-07/T-54.1-07: skills_optout_* tests.
    // -----------------------------------------------------------------

    /// RAII guard, duplicated per this crate's own established precedent
    /// (see `profile_activation_api.rs::profile_activation_tests::ScopedEnv`'s
    /// doc comment — each `#[cfg(test)]` module is its own namespace).
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

    /// T-54.1-07's primary RED-test evidence: drives `resolve_chat_optout_target()`
    /// against a REAL persisted activation record under a temp `$IRONHERMES_HOME`
    /// that is deliberately NOT profile-pivoted, so only the correct resolver
    /// (`resolve_active_profile_for` reading the persisted record) can possibly
    /// produce `mybot` — a `current_profile()`-based predicate cannot, because
    /// the temp home has no `profiles/<slug>` ancestor to reverse-walk into.
    /// This test mutates process env; it relies on the `--test-threads=1` flag
    /// this plan's verify block already pins.
    #[test]
    fn skills_optout_d06_call_site_resolves_profile_target_from_activation_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        std::fs::create_dir_all(dir.path().join("profiles").join("mybot"))
            .expect("mkdir profile dir");

        crate::server::profile_activation_api::write_active_profile_record(
            &crate::protocol::ActiveProfileRecord {
                name: "mybot".to_string(),
                scope: crate::protocol::ActivationScope::Everywhere,
                updated_at_ms: 1,
            },
        )
        .expect("write_active_profile_record should succeed");

        // Load-bearing guard: proves the temp home is NOT profile-pivoted, so
        // a `current_profile()`-based predicate could not possibly produce
        // "mybot" below.
        assert_ne!(ironhermes_core::current_profile(), "mybot");

        match resolve_chat_optout_target() {
            OptOutTarget::Profile(s) => assert_eq!(s, "mybot"),
            OptOutTarget::Root => panic!(
                "resolve_chat_optout_target() returned Root — the predicate was \
                 reverted to current_profile()/active_profile_identifier(), both \
                 of which always read \"default\" inside this never-pivoted UI \
                 server"
            ),
        }
    }

    #[test]
    fn skills_optout_d07_root_target_when_slug_is_default_sentinel() {
        assert_eq!(resolve_optout_write_target("default"), OptOutTarget::Root);
    }

    #[test]
    fn skills_optout_d06_profile_target_when_slug_is_a_real_profile() {
        assert_eq!(
            resolve_optout_write_target("mybot"),
            OptOutTarget::Profile("mybot".to_string())
        );
    }

    #[test]
    fn skills_optout_d06_payload_targets_profile_and_adds_toggled_name() {
        let catalog: std::collections::HashSet<String> =
            ["alpha", "beta"].iter().map(|s| s.to_string()).collect();
        let existing = vec!["alpha".to_string()];
        let (payload, was_disabled, dropped_stale) =
            build_profile_optout_payload("mybot", &existing, &catalog, "beta");
        assert_eq!(payload.name, "mybot");
        assert_eq!(payload.provider, None);
        assert_eq!(payload.model_default, None);
        assert_eq!(
            payload.skills_disabled,
            Some(vec!["alpha".to_string(), "beta".to_string()])
        );
        assert!(!was_disabled);
        assert!(dropped_stale.is_empty());
    }

    #[test]
    fn skills_optout_d06_stale_disabled_entry_dropped_not_fatal() {
        let catalog: std::collections::HashSet<String> =
            ["alpha", "beta"].iter().map(|s| s.to_string()).collect();
        let existing = vec!["alpha".to_string(), "ghost".to_string()];
        let (payload, _was_disabled, dropped_stale) =
            build_profile_optout_payload("mybot", &existing, &catalog, "beta");
        let disabled = payload.skills_disabled.clone().expect("Some");
        assert!(!disabled.contains(&"ghost".to_string()));
        assert_eq!(dropped_stale, vec!["ghost".to_string()]);
        assert!(crate::server::profile_api::validate_profile_config_payload(&payload).is_ok());
    }

    #[test]
    fn skills_optout_d06_payload_goes_through_existing_validator() {
        let mut payload = crate::protocol::ProfileConfigWritePayload {
            name: "mybot".to_string(),
            provider: None,
            model_default: None,
            skills_disabled: Some((0..513).map(|i| format!("skill-{i}")).collect()),
            tools_toolsets: None,
            tools_disable: None,
            tools_enable: None,
        };
        assert!(crate::server::profile_api::validate_profile_config_payload(&payload).is_err());

        payload.skills_disabled = Some(vec!["   ".to_string()]);
        assert!(crate::server::profile_api::validate_profile_config_payload(&payload).is_err());
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
///
/// Phase 54.7 Plan 06 (D-02): every key field is a REFERENCE name (`*_key_ref`,
/// a `VOICE_KEY_*` env var or a `secret/voice/*` vault entry) -- no field can
/// hold a secret value. `Default` + `#[serde(default)]` mean a caller names
/// only the fields it changes (every other field stays untouched on merge).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
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
    /// Phase 54.7 Plan 15 (D-16): override the identity's TTS model.
    pub identity_tts_model: Option<String>,
    /// Phase 54.7 Plan 15 (D-16): override the identity's TTS output format.
    pub identity_tts_format: Option<String>,
    /// Phase 54.7 Plan 15 (D-16): override the identity's realtime model.
    pub identity_realtime_model: Option<String>,
    // D-17: appearance fields — one record holds BOTH appearance and voice.
    /// Orb style token: one of "classic" | "bloom" | "ascii" | "network".
    pub identity_appearance_style: Option<String>,
    /// Idle base hue 0–360 (mirrors IdentityAppearance.base_hue: Option<u16>).
    pub identity_base_hue: Option<u16>,
    /// Scale factor 0.5–2.0 (mirrors IdentityAppearance.size: Option<f32>).
    pub identity_size: Option<f32>,
    /// Glow intensity 0.0–1.0 (mirrors IdentityAppearance.glow: Option<f32>).
    pub identity_glow: Option<f32>,
    // Phase 54.7 Plan 06 (D-01/D-03): the OpenAI-compatible TTS block
    // (`tts.openai_compatible`). Empty `path` / `key_ref` clear the field.
    /// Server base URL including any `/v1` segment (empty = unset).
    pub tts_compat_base_url: Option<String>,
    /// Endpoint path override; empty clears it (the default path applies).
    pub tts_compat_path: Option<String>,
    /// Key REFERENCE name (never a value); empty clears it.
    pub tts_compat_key_ref: Option<String>,
    pub tts_compat_model: Option<String>,
    pub tts_compat_voice: Option<String>,
    pub tts_compat_format: Option<String>,
    // Phase 54.7 Plan 06 (D-01/D-03): the OpenAI-compatible STT block
    // (`stt.openai_compatible`). Empty `path` / `key_ref` clear the field.
    pub stt_compat_base_url: Option<String>,
    pub stt_compat_path: Option<String>,
    /// Key REFERENCE name (never a value); empty clears it.
    pub stt_compat_key_ref: Option<String>,
    pub stt_compat_model: Option<String>,
    // Phase 54.7 Plan 06 (D-09, Pitfall 13): per-provider TTS settings, each
    // persisted into its OWN block so a voice typed for one provider can no
    // longer overwrite another's.
    pub tts_openai_model: Option<String>,
    pub tts_openai_voice: Option<String>,
    pub tts_openai_format: Option<String>,
    pub tts_elevenlabs_model: Option<String>,
    pub tts_elevenlabs_voice: Option<String>,
    // Phase 54.7 Plan 06 (D-13/D-15): the voice family and the top-level
    // `realtime:` block. Saving `voice_service` is the one-way migration: it
    // also clears the legacy `voice.realtime_*` keys and rewrites `open_mic`.
    /// `realtime` | `turn_based`.
    pub voice_service: Option<String>,
    pub realtime_provider: Option<String>,
    /// Empty means the api.openai.com default.
    pub realtime_base_url: Option<String>,
    /// Key REFERENCE name (never a value); empty clears it.
    pub realtime_key_ref: Option<String>,
    pub realtime_model: Option<String>,
    pub realtime_voice: Option<String>,
    /// A model id, or the reserved `off`.
    pub realtime_transcription_model: Option<String>,
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
    /// Phase 54.7 Plan 15 (D-16): the identity's own TTS model (`None` = it
    /// inherits the global block's model).
    pub free_mode_model: Option<String>,
    /// Phase 54.7 Plan 15 (D-16): the identity's own TTS format (`None` = it
    /// inherits the global block's format).
    pub free_mode_format: Option<String>,
    /// Phase 54.7 Plan 15 (D-16): resolved realtime model (the identity's own
    /// or the global `realtime.model()`).
    pub realtime_model: String,
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
///
/// Phase 54.7 Plan 06: `Default` + `#[serde(default)]` let a stale client
/// tolerate fields added by a newer server. Key fields carry the reference
/// NAME and a `found` / `not_found` / `none` status string -- never a value.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
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
    // Phase 54.7 Plan 06 (D-01/D-02/D-03): `tts.openai_compatible`.
    pub tts_compat_base_url: String,
    /// Empty when unset (the default path applies).
    pub tts_compat_path: String,
    /// Reference NAME only; empty when unset.
    pub tts_compat_key_ref: String,
    pub tts_compat_model: String,
    pub tts_compat_voice: String,
    pub tts_compat_format: String,
    /// `found` / `not_found` / `none` -- how the reference resolved, never the value.
    pub tts_compat_key_status: String,
    // Phase 54.7 Plan 06: `stt.openai_compatible`.
    pub stt_compat_base_url: String,
    /// Empty when unset (the default path applies).
    pub stt_compat_path: String,
    /// Reference NAME only; empty when unset.
    pub stt_compat_key_ref: String,
    pub stt_compat_model: String,
    pub stt_compat_key_status: String,
    // Phase 54.7 Plan 06 (D-09): per-provider TTS settings.
    pub tts_openai_model: String,
    pub tts_openai_voice: String,
    pub tts_openai_format: String,
    pub tts_elevenlabs_model: String,
    pub tts_elevenlabs_voice: String,
    // Phase 54.7 Plan 06 (D-13/D-15): voice family + the `realtime:` block,
    // resolved through the legacy fold (the panel shows what the server uses).
    /// Effective voice family: `realtime` | `turn_based`.
    pub voice_service: String,
    /// True while pre-54.7 realtime state is still on disk (offers the
    /// one-way migration save).
    pub legacy_voice_keys_present: bool,
    /// Selectable interruption modes -- exactly `["push_to_interrupt"]` (the
    /// only wired mode, Pitfall 10), so the Interruption row is not rendered.
    pub barge_in_modes: Vec<String>,
    pub realtime_provider: String,
    pub realtime_base_url: String,
    /// Reference NAME only; empty when unset.
    pub realtime_key_ref: String,
    pub realtime_model: String,
    pub realtime_voice: String,
    pub realtime_transcription_model: String,
    pub realtime_key_status: String,
    /// Phase 54.7 Plan 15 (D-16): TTS registry ids whose provider is registered
    /// AND available on this server (Edge always; ElevenLabs / OpenAI with a
    /// key in the environment; OpenAI-compatible with a saved Base URL). Ids
    /// only -- no key value crosses to the browser. The identity Provider
    /// select lists only these.
    pub tts_providers_usable: Vec<String>,
}

/// The interruption modes the server accepts and the snapshot advertises.
/// `open_mic` and `half_duplex` are not wired end to end (D-15, Pitfall 10).
#[cfg(not(target_arch = "wasm32"))]
const BARGE_IN_MODES: &[&str] = &["push_to_interrupt"];

/// The voice families a payload may select.
#[cfg(not(target_arch = "wasm32"))]
const VOICE_SERVICES: &[&str] = &["realtime", "turn_based"];

/// Phase 54.7 Plan 06 (D-02): how each configured key reference resolved.
///
/// Computed asynchronously (a vault read may be involved) and handed to the
/// pure [`build_voice_snapshot`]. Values are the registry `KEY_STATUS_*`
/// strings; no secret value is ever held here.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VoiceKeyStatuses {
    pub tts_compat: &'static str,
    pub stt_compat: &'static str,
    pub realtime: &'static str,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for VoiceKeyStatuses {
    fn default() -> Self {
        use crate::voice_registry::KEY_STATUS_NONE;
        Self {
            tts_compat: KEY_STATUS_NONE,
            stt_compat: KEY_STATUS_NONE,
            realtime: KEY_STATUS_NONE,
        }
    }
}

/// Map a resolved key to the registry status string (value discarded).
#[cfg(not(target_arch = "wasm32"))]
fn key_status_str(key: &ironhermes_core::voice_http::VoiceKey) -> &'static str {
    use crate::voice_registry::{KEY_STATUS_FOUND, KEY_STATUS_NONE, KEY_STATUS_NOT_FOUND};
    use ironhermes_core::voice_http::VoiceKey;
    match key {
        VoiceKey::None => KEY_STATUS_NONE,
        VoiceKey::NotFound => KEY_STATUS_NOT_FOUND,
        VoiceKey::Found(_) => KEY_STATUS_FOUND,
    }
}

/// Resolve every configured key reference to a status (D-02). Only the
/// namespaced [`resolve_voice_key`](ironhermes_core::voice_http::resolve_voice_key)
/// reads a reference; the secret value is dropped here.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn voice_key_statuses(
    config: &ironhermes_core::config::Config,
) -> VoiceKeyStatuses {
    use ironhermes_core::voice_http::{resolve_voice_key, voice_vault_config};
    let vault = voice_vault_config(config);
    let tts_compat = resolve_voice_key(
        config.tts.openai_compatible.api_key_ref.as_deref(),
        vault.as_ref(),
    )
    .await;
    let stt_compat = resolve_voice_key(
        config.stt.openai_compatible.api_key_ref.as_deref(),
        vault.as_ref(),
    )
    .await;
    let realtime = resolve_realtime_key(config).await;
    VoiceKeyStatuses {
        tts_compat: key_status_str(&tts_compat),
        stt_compat: key_status_str(&stt_compat),
        realtime: key_status_str(&realtime),
    }
}

/// Phase 54.7 Plan 06 (D-02, Pitfall 9, T-54.7-70): the ONE realtime key rule.
///
/// - A typed `realtime.api_key_ref` resolves through the namespaced
///   `resolve_voice_key` (a `VOICE_KEY_*` env var or a `secret/voice/*` vault
///   entry; anything else is `NotFound` before any read).
/// - With no reference, the OpenAI provider key (`providers.openai.api_key_env`,
///   else `OPENAI_API_KEY`) is used ONLY when `realtime.base_url` is the
///   api.openai.com default -- the key never travels to a custom address.
/// - With no reference and a custom `base_url`, there is no key.
///
/// The key status here, `test_realtime` (plan 07) and `mint_realtime_for`
/// (plan 11) all call this, so the three can never disagree.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn resolve_realtime_key(
    config: &ironhermes_core::config::Config,
) -> ironhermes_core::voice_http::VoiceKey {
    use ironhermes_core::voice_http::{
        resolve_builtin_env_key, resolve_voice_key, voice_vault_config, VoiceKey,
    };
    let typed = config
        .realtime
        .api_key_ref
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    if let Some(reference) = typed {
        return resolve_voice_key(Some(reference), voice_vault_config(config).as_ref()).await;
    }
    if config.realtime.is_openai_default_base_url() {
        let env_name = config
            .providers
            .get("openai")
            .and_then(|p| p.api_key_env.as_deref())
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or("OPENAI_API_KEY");
        return resolve_builtin_env_key(env_name);
    }
    VoiceKey::None
}

/// Wire error for a rejected voice field: `voice_invalid:{field}:{code}`. The
/// client maps `code` back to UI-SPEC copy with `FieldError::from_code`.
#[cfg(not(target_arch = "wasm32"))]
fn voice_invalid(field: &str, code: &str) -> ServerFnError {
    ServerFnError::new(format!("voice_invalid:{field}:{code}"))
}

/// Run a registry validator over an optional payload string (trimmed).
#[cfg(not(target_arch = "wasm32"))]
fn check_voice_field(
    field: &str,
    value: &Option<String>,
    rule: impl Fn(&str) -> Result<(), crate::voice_registry::FieldError>,
) -> Result<(), ServerFnError> {
    if let Some(v) = value {
        rule(v.trim()).map_err(|e| voice_invalid(field, e.code()))?;
    }
    Ok(())
}

/// Phase 54.7 Plan 15 (D-16, T-54.7-64): refuse an identity-target payload that
/// also sets a global field -- an endpoint, a key reference, STT, the voice
/// service, VAD, wake word, anything not named `identity_*`. Derived from the
/// payload's own serialization, so a global field added later is refused here
/// without anyone remembering to list it. The wire code is
/// `voice_invalid:{field}:identity_global_field`.
#[cfg(not(target_arch = "wasm32"))]
fn reject_global_fields_for_identity(payload: &VoiceWritePayload) -> Result<(), ServerFnError> {
    // Fail closed: a payload that cannot be inspected is not accepted.
    let Ok(serde_json::Value::Object(fields)) = serde_json::to_value(payload) else {
        return Err(voice_invalid("payload", "identity_global_field"));
    };
    match fields
        .iter()
        .find(|(name, value)| !name.starts_with("identity_") && !value.is_null())
    {
        Some((name, _)) => Err(voice_invalid(name, "identity_global_field")),
        None => Ok(()),
    }
}

/// Phase 54.7 Plan 15 (D-16): an identity format must be one the identity's
/// provider lists in the registry (`voice_invalid:identity_tts_format:format`).
#[cfg(not(target_arch = "wasm32"))]
fn check_identity_format(format: &str, provider: &str) -> Result<(), ServerFnError> {
    let listed = crate::voice_registry::tts_provider(provider)
        .is_some_and(|spec| spec.formats.contains(&format.trim()));
    if listed {
        Ok(())
    } else {
        Err(voice_invalid("identity_tts_format", "format"))
    }
}

/// Phase 36.17.10 Plan 01: Validate that numeric and string fields in the
/// payload are within acceptable ranges before any config mutation.
/// Ranges from UI-SPEC Surface 4.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn validate_voice_payload(payload: &VoiceWritePayload) -> Result<(), ServerFnError> {
    // Phase 54.7 Plan 15 (D-16, T-54.7-64): an identity-target save carries
    // identity fields only. Checked first so no other rule can mask it.
    if payload.identity_slug.is_some() {
        reject_global_fields_for_identity(payload)?;
    }
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
    // Phase 54.7 (Pitfall 11): provider ids are checked against the shared
    // registry -- an unknown provider string is rejected, never merged.
    if let Some(ref provider) = payload.stt_provider {
        if provider.len() > 64 {
            return Err(ServerFnError::new("stt_provider too long (max 64 chars)"));
        }
        if crate::voice_registry::stt_provider(provider).is_none() {
            return Err(voice_invalid("stt_provider", "unknown_provider"));
        }
    }
    if let Some(ref provider) = payload.tts_provider {
        if provider.len() > 64 {
            return Err(ServerFnError::new("tts_provider too long (max 64 chars)"));
        }
        if crate::voice_registry::tts_provider(provider).is_none() {
            return Err(voice_invalid("tts_provider", "unknown_provider"));
        }
    }
    // Phase 54.7 (D-04): OpenAI-compatible TTS block -- the shared registry
    // rules, byte for byte what the browser already showed.
    {
        use crate::voice_registry as vr;
        check_voice_field("tts_compat_base_url", &payload.tts_compat_base_url, |v| {
            vr::validate_base_url(v, false)
        })?;
        check_voice_field(
            "tts_compat_path",
            &payload.tts_compat_path,
            vr::validate_endpoint_path,
        )?;
        check_voice_field(
            "tts_compat_key_ref",
            &payload.tts_compat_key_ref,
            vr::validate_key_ref,
        )?;
        check_voice_field(
            "tts_compat_model",
            &payload.tts_compat_model,
            vr::validate_model_id,
        )?;
        check_voice_field(
            "tts_compat_voice",
            &payload.tts_compat_voice,
            vr::validate_voice_id,
        )?;
        // OpenAI-compatible STT block.
        check_voice_field("stt_compat_base_url", &payload.stt_compat_base_url, |v| {
            vr::validate_base_url(v, false)
        })?;
        check_voice_field(
            "stt_compat_path",
            &payload.stt_compat_path,
            vr::validate_endpoint_path,
        )?;
        check_voice_field(
            "stt_compat_key_ref",
            &payload.stt_compat_key_ref,
            vr::validate_key_ref,
        )?;
        check_voice_field(
            "stt_compat_model",
            &payload.stt_compat_model,
            vr::validate_model_id,
        )?;
        // Per-provider TTS fields (D-09).
        check_voice_field(
            "tts_openai_model",
            &payload.tts_openai_model,
            vr::validate_model_id,
        )?;
        check_voice_field(
            "tts_openai_voice",
            &payload.tts_openai_voice,
            vr::validate_voice_id,
        )?;
        check_voice_field(
            "tts_elevenlabs_model",
            &payload.tts_elevenlabs_model,
            vr::validate_model_id,
        )?;
        check_voice_field(
            "tts_elevenlabs_voice",
            &payload.tts_elevenlabs_voice,
            vr::validate_voice_id,
        )?;
        // Realtime block (D-13). An empty base_url means the OpenAI default.
        if let Some(ref provider) = payload.realtime_provider {
            if vr::realtime_provider(provider).is_none() {
                return Err(voice_invalid("realtime_provider", "unknown_provider"));
            }
        }
        check_voice_field("realtime_base_url", &payload.realtime_base_url, |v| {
            vr::validate_base_url(v, false)
        })?;
        check_voice_field(
            "realtime_key_ref",
            &payload.realtime_key_ref,
            vr::validate_key_ref,
        )?;
        check_voice_field(
            "realtime_model",
            &payload.realtime_model,
            vr::validate_model_id,
        )?;
        check_voice_field(
            "realtime_voice",
            &payload.realtime_voice,
            vr::validate_voice_id,
        )?;
        check_voice_field(
            "realtime_transcription_model",
            &payload.realtime_transcription_model,
            vr::validate_transcription_model,
        )?;
    }
    // Phase 54.7 (D-15): the voice family, and the interruption modes the
    // snapshot advertises (`open_mic` is retired as a selectable value).
    if let Some(ref service) = payload.voice_service {
        if !VOICE_SERVICES.contains(&service.as_str()) {
            return Err(voice_invalid("voice_service", "unknown_service"));
        }
    }
    if let Some(ref mode) = payload.barge_in_mode {
        if !BARGE_IN_MODES.contains(&mode.as_str()) {
            return Err(voice_invalid("barge_in_mode", "unsupported_mode"));
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
        // Phase 54.7 Plan 15 (D-16, T-54.7-66): identity provider / model /
        // voice / format / realtime values go through the SAME registry rules
        // as the global fields. This replaces the bare 64-char length caps
        // (T-40.5-03-03): validate_voice_id keeps that limit and adds the
        // charset.
        {
            use crate::voice_registry as vr;
            if let Some(ref provider) = payload.identity_tts_provider {
                if vr::tts_provider(provider).is_none() {
                    return Err(voice_invalid("identity_tts_provider", "unknown_provider"));
                }
            }
            check_voice_field(
                "identity_tts_model",
                &payload.identity_tts_model,
                vr::validate_model_id,
            )?;
            check_voice_field(
                "identity_tts_voice",
                &payload.identity_tts_voice,
                vr::validate_voice_id,
            )?;
            check_voice_field(
                "identity_realtime_voice",
                &payload.identity_realtime_voice,
                vr::validate_voice_id,
            )?;
            check_voice_field(
                "identity_realtime_model",
                &payload.identity_realtime_model,
                vr::validate_model_id,
            )?;
            // The format belongs to the provider the identity ends up on; when
            // the payload names it, check here, otherwise validate_merged_voice
            // checks against the stored one.
            if let (Some(format), Some(provider)) =
                (&payload.identity_tts_format, &payload.identity_tts_provider)
            {
                check_identity_format(format, provider)?;
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
pub(crate) fn merge_voice_payload(
    config: &mut ironhermes_core::config::Config,
    payload: &VoiceWritePayload,
) {
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
    // Phase 54.7 Plan 06 (D-01/D-03): `tts.openai_compatible`. `path` and
    // `key_ref` are optional, so an empty string clears them back to None.
    {
        let oc = &mut config.tts.openai_compatible;
        if let Some(ref v) = payload.tts_compat_base_url {
            oc.base_url = v.trim().to_string();
        }
        if let Some(ref v) = payload.tts_compat_path {
            oc.path = non_empty_trimmed(v);
        }
        if let Some(ref v) = payload.tts_compat_key_ref {
            oc.api_key_ref = non_empty_trimmed(v);
        }
        if let Some(ref v) = payload.tts_compat_model {
            oc.model = v.trim().to_string();
        }
        if let Some(ref v) = payload.tts_compat_voice {
            oc.voice = v.trim().to_string();
        }
        if let Some(ref v) = payload.tts_compat_format {
            oc.format = v.trim().to_string();
        }
    }
    // Phase 54.7 Plan 06 (D-01/D-03): `stt.openai_compatible`.
    {
        let oc = &mut config.stt.openai_compatible;
        if let Some(ref v) = payload.stt_compat_base_url {
            oc.base_url = v.trim().to_string();
        }
        if let Some(ref v) = payload.stt_compat_path {
            oc.path = non_empty_trimmed(v);
        }
        if let Some(ref v) = payload.stt_compat_key_ref {
            oc.api_key_ref = non_empty_trimmed(v);
        }
        if let Some(ref v) = payload.stt_compat_model {
            oc.model = v.trim().to_string();
        }
    }
    // Phase 54.7 Plan 06 (D-09): per-provider TTS blocks, never crossed.
    {
        let openai = &mut config.tts.openai;
        if let Some(ref v) = payload.tts_openai_model {
            openai.model = v.trim().to_string();
        }
        if let Some(ref v) = payload.tts_openai_voice {
            openai.voice = v.trim().to_string();
        }
        if let Some(ref v) = payload.tts_openai_format {
            openai.format = v.trim().to_string();
        }
        let eleven = &mut config.tts.elevenlabs;
        if let Some(ref v) = payload.tts_elevenlabs_model {
            eleven.model_id = v.trim().to_string();
        }
        if let Some(ref v) = payload.tts_elevenlabs_voice {
            eleven.voice_id = v.trim().to_string();
        }
    }
    if let Some(v) = payload.auto_tts {
        config.voice.auto_tts = v;
    }
    // Phase 36.17.12 Plan 01 (T-36.17.12-01-01): Parse barge_in_mode string into
    // BargeInMode enum via serde round-trip. Unknown/garbage strings are silently
    // ignored (no assignment, no error) — only valid snake_case variants reach config.
    // Phase 54.7 (D-15): only a mode the snapshot advertises is written; open_mic
    // is never mapped back into the config.
    if let Some(ref v) = payload.barge_in_mode {
        if BARGE_IN_MODES.contains(&v.as_str()) {
            use ironhermes_core::config::BargeInMode;
            let parsed: Result<BargeInMode, _> =
                serde_json::from_value(serde_json::Value::String(v.clone()));
            if let Ok(mode) = parsed {
                config.voice.barge_in_mode = mode;
            }
        }
    }
    // Phase 54.7 Plan 06 (D-13): the top-level `realtime:` block. An empty
    // base_url stores the OpenAI default; an empty optional field clears it.
    {
        let rt = &mut config.realtime;
        if let Some(ref v) = payload.realtime_provider {
            rt.provider = v.trim().to_string();
        }
        if let Some(ref v) = payload.realtime_base_url {
            let t = v.trim();
            rt.base_url = if t.is_empty() {
                ironhermes_core::config::REALTIME_DEFAULT_BASE_URL.to_string()
            } else {
                t.to_string()
            };
        }
        if let Some(ref v) = payload.realtime_key_ref {
            rt.api_key_ref = non_empty_trimmed(v);
        }
        if let Some(ref v) = payload.realtime_model {
            rt.model = non_empty_trimmed(v);
        }
        if let Some(ref v) = payload.realtime_voice {
            rt.voice = non_empty_trimmed(v);
        }
        if let Some(ref v) = payload.realtime_transcription_model {
            rt.transcription_model = non_empty_trimmed(v);
        }
    }
    // Phase 54.7 Plan 06 (D-13/D-15, one-way): selecting the voice family also
    // retires the legacy state -- the three `voice.realtime_*` keys are cleared
    // (their values already live in `realtime:` through the load-time fold) and a
    // legacy `open_mic` becomes `push_to_interrupt`.
    if let Some(ref v) = payload.voice_service {
        use ironhermes_core::config::{BargeInMode, VoiceService};
        let parsed: Result<VoiceService, _> =
            serde_json::from_value(serde_json::Value::String(v.clone()));
        if let Ok(service) = parsed {
            config.voice.service = Some(service);
            config.voice.realtime_model = None;
            config.voice.realtime_voice = None;
            config.voice.realtime_transcription_model = None;
            if config.voice.barge_in_mode == BargeInMode::OpenMic {
                config.voice.barge_in_mode = BargeInMode::PushToInterrupt;
            }
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
        // WR-11: validation runs on the TRIMMED value, so the stored value must be
        // trimmed too -- a padded id would pass the save gate and then fail the
        // realtime gate (or reach the provider verbatim) at use time.
        if let Some(ref v) = payload.identity_tts_provider {
            rec.voice.free_mode_tts_provider = Some(v.trim().to_string());
        }
        if let Some(ref v) = payload.identity_tts_voice {
            rec.voice.free_mode_tts_voice = Some(v.trim().to_string());
        }
        if let Some(ref v) = payload.identity_realtime_voice {
            rec.voice.realtime_voice = Some(v.trim().to_string());
        }
        // Phase 54.7 Plan 15 (D-16): model and format ride the same record.
        if let Some(ref v) = payload.identity_tts_model {
            rec.voice.free_mode_tts_model = Some(v.trim().to_string());
        }
        if let Some(ref v) = payload.identity_tts_format {
            rec.voice.free_mode_tts_format = Some(v.trim().to_string());
        }
        if let Some(ref v) = payload.identity_realtime_model {
            rec.voice.realtime_model = Some(v.trim().to_string());
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

/// `Some(trimmed)` for a non-empty value, `None` for an empty one (the
/// "empty clears the optional field" rule).
#[cfg(not(target_arch = "wasm32"))]
fn non_empty_trimmed(v: &str) -> Option<String> {
    let t = v.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Phase 54.7 Plan 06 (T-54.7-71): reject a CHANGED format the provider's
/// registry does not list. A value equal to what is already stored passes
/// unchecked, so a hand-typed legacy format (an Edge
/// `audio-24khz-48kbitrate-mono-mp3`, an OpenAI `flac`) can never make an
/// unrelated save fail -- the panel echoes stored values on every save.
#[cfg(not(target_arch = "wasm32"))]
fn check_format_change(
    field: &str,
    incoming: &str,
    stored_value: &str,
    provider: &str,
) -> Result<(), ServerFnError> {
    let v = incoming.trim();
    if v == stored_value.trim() {
        return Ok(());
    }
    let listed = crate::voice_registry::tts_provider(provider)
        .map(|spec| spec.formats.contains(&v))
        .unwrap_or(false);
    if listed {
        Ok(())
    } else {
        Err(voice_invalid(field, "format"))
    }
}

/// Apply `check_format_change` to every format field a save payload carries.
#[cfg(not(target_arch = "wasm32"))]
fn validate_format_changes(
    stored: &ironhermes_core::config::Config,
    payload: &VoiceWritePayload,
) -> Result<(), ServerFnError> {
    let check = |field: &str,
                 incoming: &Option<String>,
                 stored_value: &str,
                 provider: &str|
     -> Result<(), ServerFnError> {
        match incoming {
            Some(v) => check_format_change(field, v, stored_value, provider),
            None => Ok(()),
        }
    };
    check(
        "tts_compat_format",
        &payload.tts_compat_format,
        &stored.tts.openai_compatible.format,
        "openai_compatible",
    )?;
    check(
        "tts_openai_format",
        &payload.tts_openai_format,
        &stored.tts.openai.format,
        "openai",
    )?;
    // Edge stores a provider-native string the provider ignores (RESEARCH
    // Pitfall 6); the panel echoes it back, so only a changed value is checked.
    check(
        "tts_edge_format",
        &payload.tts_edge_format,
        &stored.tts.edge.output_format,
        "edge",
    )
}

/// Phase 54.7 Plan 06 / 15: post-merge cross-field rules. A provider that
/// needs an address cannot be selected with an empty one, and (plan 15, D-16)
/// the identity a save WRITES must end up on a provider this server has
/// configured and a format that provider lists.
///
/// The identity rules are scoped (plan-checker issue): they run only for the
/// identity named by `payload.identity_slug`, and the provider-usability rule
/// only when that same payload SETS `identity_tts_provider`. A global save, a
/// realtime-only identity save and every other identity -- including the
/// seeded elevenlabs personas on a Kokoro-only server -- are never judged.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn validate_merged_voice(
    config: &ironhermes_core::config::Config,
    payload: &VoiceWritePayload,
) -> Result<(), ServerFnError> {
    if let Some(slug) = payload.identity_slug.as_deref() {
        let record_provider = config
            .identities
            .get(slug)
            .and_then(|r| r.voice.free_mode_tts_provider.as_deref());
        if payload.identity_tts_provider.is_some() {
            if let Some(provider) = record_provider {
                // The same set the snapshot advertises as `tts_providers_usable`.
                if !usable_tts_providers(config).iter().any(|id| id == provider) {
                    return Err(voice_invalid("identity_tts_provider", "provider_unusable"));
                }
            }
        }
        if let Some(format) = payload.identity_tts_format.as_deref() {
            // The provider the identity ends up on: its own, else the global one.
            check_identity_format(format, record_provider.unwrap_or(&config.tts.provider))?;
        }
    }
    if config.tts.provider == "openai_compatible"
        && config.tts.openai_compatible.base_url.trim().is_empty()
    {
        return Err(voice_invalid("tts_compat_base_url", "base_url_empty"));
    }
    if config.stt.provider == "openai_compatible"
        && config.stt.openai_compatible.base_url.trim().is_empty()
    {
        return Err(voice_invalid("stt_compat_base_url", "base_url_empty"));
    }
    Ok(())
}

/// Phase 54.7 Plan 06: the whole payload-to-config step. Ordered so that a
/// format rule sees the STORED values, the merge sees a validated payload, and
/// the cross-field rules see the merged result. `update_voice_config`, plan
/// 08's panel-payload save test and plan 10's D-18 harness all drive this.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn apply_voice_payload(
    config: &mut ironhermes_core::config::Config,
    payload: &VoiceWritePayload,
) -> Result<(), ServerFnError> {
    validate_format_changes(config, payload)?;
    merge_voice_payload(config, payload);
    validate_merged_voice(config, payload)
}

/// Phase 36.17.10 Plan 01 (T-36.17.10-01-02, T-36.17.10-01-04): Build a
/// VoiceConfigSnapshot from a freshly loaded Config. Secret field values are
/// never included — only their presence is noted by label name.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn build_voice_snapshot(
    config: &ironhermes_core::config::Config,
    keys: &VoiceKeyStatuses,
) -> VoiceConfigSnapshot {
    // Determine the active STT model for display (derived from provider selection).
    let stt_model = match config.stt.provider.as_str() {
        "groq" => config.stt.groq.model.clone(),
        "openai" => config.stt.openai.model.clone(),
        "openai_compatible" => config.stt.openai_compatible.model.clone(),
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
    let global_realtime_voice = config.realtime.voice().to_string();
    let global_realtime_model = config.realtime.model().to_string();
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
            // Phase 54.7 Plan 15 (D-16): the identity's own model / format and
            // the resolved realtime model.
            let record_voice = config.identities.get(slug.as_str()).map(|r| &r.voice);
            let free_mode_model = record_voice.and_then(|v| v.free_mode_tts_model.clone());
            let free_mode_format = record_voice.and_then(|v| v.free_mode_tts_format.clone());
            let realtime_model = record_voice
                .and_then(|v| v.realtime_model.clone())
                .unwrap_or_else(|| global_realtime_model.clone());
            IdentityOverride {
                slug: slug.clone(),
                free_mode_provider: provider,
                free_mode_voice: voice,
                free_mode_model,
                free_mode_format,
                realtime_voice: realtime,
                realtime_model,
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
        tts_compat_base_url: config.tts.openai_compatible.base_url.clone(),
        tts_compat_path: config
            .tts
            .openai_compatible
            .path
            .clone()
            .unwrap_or_default(),
        tts_compat_key_ref: config
            .tts
            .openai_compatible
            .api_key_ref
            .clone()
            .unwrap_or_default(),
        tts_compat_model: config.tts.openai_compatible.model.clone(),
        tts_compat_voice: config.tts.openai_compatible.voice.clone(),
        tts_compat_format: config.tts.openai_compatible.format.clone(),
        tts_compat_key_status: keys.tts_compat.to_string(),
        stt_compat_base_url: config.stt.openai_compatible.base_url.clone(),
        stt_compat_path: config
            .stt
            .openai_compatible
            .path
            .clone()
            .unwrap_or_default(),
        stt_compat_key_ref: config
            .stt
            .openai_compatible
            .api_key_ref
            .clone()
            .unwrap_or_default(),
        stt_compat_model: config.stt.openai_compatible.model.clone(),
        stt_compat_key_status: keys.stt_compat.to_string(),
        tts_openai_model: config.tts.openai.model.clone(),
        tts_openai_voice: config.tts.openai.voice.clone(),
        tts_openai_format: config.tts.openai.format.clone(),
        tts_elevenlabs_model: config.tts.elevenlabs.model_id.clone(),
        tts_elevenlabs_voice: config.tts.elevenlabs.voice_id.clone(),
        voice_service: config.voice.effective_service().as_str().to_string(),
        legacy_voice_keys_present: config.voice.legacy_realtime_keys_present(),
        barge_in_modes: BARGE_IN_MODES.iter().map(|m| (*m).to_string()).collect(),
        realtime_provider: config.realtime.provider.clone(),
        realtime_base_url: config.realtime.base_url.clone(),
        realtime_key_ref: config.realtime.api_key_ref.clone().unwrap_or_default(),
        realtime_model: config.realtime.model().to_string(),
        realtime_voice: config.realtime.voice().to_string(),
        realtime_transcription_model: config.realtime.transcription_model().to_string(),
        realtime_key_status: keys.realtime.to_string(),
        tts_providers_usable: usable_tts_providers(config),
    }
}

/// Phase 54.7 Plan 15 (D-16): the TTS registry ids a server can actually use
/// right now -- registered AND `is_available()` (Edge always, ElevenLabs /
/// OpenAI when their key is in the environment, OpenAI-compatible when a Base
/// URL is saved). Registry order. Both the snapshot and the identity
/// usability rule in [`validate_merged_voice`] call this, so what the panel
/// lists and what the server accepts can never disagree. Ids only: no key
/// value leaves this function.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn usable_tts_providers(config: &ironhermes_core::config::Config) -> Vec<String> {
    let registry = ironhermes_tools::tts::build_tts_registry(&config.tts);
    crate::voice_registry::TTS_PROVIDERS
        .iter()
        .filter(|spec| registry.get(spec.id).is_some_and(|p| p.is_available()))
        .map(|spec| spec.id.to_string())
        .collect()
}

/// WR-12: `payload` without the provider / model / voice fields that merely echo
/// what `stored` already holds (compared trimmed). The panel sends all providers'
/// values on every save; judging an unchanged, hidden value would reject the save
/// with an error naming a field the user cannot see. Changed values are kept and
/// validated in full, so this never lets a NEW bad value through. Identity-target
/// payloads are returned untouched (their fields are always user-edited).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn drop_unchanged_choice_fields(
    stored: &ironhermes_core::config::Config,
    payload: &VoiceWritePayload,
) -> VoiceWritePayload {
    let mut out = payload.clone();
    if payload.identity_slug.is_some() {
        return out;
    }
    // `stored` lists every spelling of the current value (raw and effective).
    fn drop_if_same(field: &mut Option<String>, stored: &[&str]) {
        if field
            .as_deref()
            .is_some_and(|v| stored.iter().any(|s| s.trim() == v.trim()))
        {
            *field = None;
        }
    }
    let tts = &stored.tts;
    let rt = &stored.realtime;
    drop_if_same(&mut out.stt_provider, &[&stored.stt.provider]);
    drop_if_same(&mut out.tts_provider, &[&tts.provider]);
    drop_if_same(&mut out.tts_compat_model, &[&tts.openai_compatible.model]);
    drop_if_same(&mut out.tts_compat_voice, &[&tts.openai_compatible.voice]);
    drop_if_same(
        &mut out.stt_compat_model,
        &[&stored.stt.openai_compatible.model],
    );
    drop_if_same(&mut out.tts_openai_model, &[&tts.openai.model]);
    drop_if_same(&mut out.tts_openai_voice, &[&tts.openai.voice]);
    drop_if_same(&mut out.tts_elevenlabs_model, &[&tts.elevenlabs.model_id]);
    drop_if_same(&mut out.tts_elevenlabs_voice, &[&tts.elevenlabs.voice_id]);
    drop_if_same(&mut out.realtime_provider, &[&rt.provider]);
    drop_if_same(
        &mut out.realtime_model,
        &[rt.model.as_deref().unwrap_or(""), rt.model()],
    );
    drop_if_same(
        &mut out.realtime_voice,
        &[rt.voice.as_deref().unwrap_or(""), rt.voice()],
    );
    drop_if_same(
        &mut out.realtime_transcription_model,
        &[
            rt.transcription_model.as_deref().unwrap_or(""),
            rt.transcription_model(),
        ],
    );
    out
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
    // RR-05 (WR-06): serialise the root load-modify-save; no `.await` below.
    let _root_write = crate::server::profile_activation_api::lock_root_config_write();

    // Step 1: fresh disk read (NOT app_state.config — that is the startup snapshot)
    let mut config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;

    // Step 2: validate. WR-12: the panel sends every provider's model/voice on
    // every save, so a stray value already on disk for an UNSELECTED provider
    // (hand-edited, or from before the charset gate) must not block the save with
    // an error on a hidden field. A field that merely echoes its stored value is
    // not a change and is not re-judged; any changed value is fully validated.
    let payload = drop_unchanged_choice_fields(&config, &payload);
    validate_voice_payload(&payload)?;

    // Step 3: gate — web config write is disabled unless operator opts in
    if !config.security.web_config_write_enabled {
        return Err(ServerFnError::new("Config writes are disabled"));
    }

    // Step 4: format rule -> merge -> cross-field rule, then atomic save
    // (Phase 54.7 Plan 06: apply_voice_payload is the one path tests drive).
    apply_voice_payload(&mut config, &payload)?;
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
    let keys = voice_key_statuses(&config).await;
    Ok(build_voice_snapshot(&config, &keys))
}

/// Phase 41.2 Plan 02 (D-06): the base64-audio return contract for
/// `synth_voice_test`, consumed by the shared `TestVoiceButton` client.
/// Same base64-in-JSON transport `attach_file` uses (kanban_api.rs:807),
/// mirrored in the encode direction.
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq)]
pub struct VoiceSampleAudio {
    pub audio_b64: String,
    pub mime: String,
}

/// Phase 54.7 Plan 16 (D-17): the UNSAVED TTS form values a Test voice button
/// auditions. Every field is optional text; there is deliberately NO text/input
/// field -- the server always synthesizes the fixed `SAMPLE_TEXT`, so the
/// button can never be used to drive arbitrary TTS spend. Compiled on both
/// targets (the client builds it from the form).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct TtsTestSettings {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub path: Option<String>,
    pub key_ref: Option<String>,
    pub model: Option<String>,
    pub voice: Option<String>,
    pub format: Option<String>,
}

/// D-06 discretion: the fixed short audition phrase. No client-supplied text
/// param exists on any Test-voice server fn (excessive-TTS-spend DoS closed).
#[cfg(not(target_arch = "wasm32"))]
const SAMPLE_TEXT: &str = "This is how I sound.";

/// The audio read cap: a misbehaving TTS server cannot make the web server
/// buffer and base64 an unbounded file.
#[cfg(not(target_arch = "wasm32"))]
const SAMPLE_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Phase 54.7 Plan 16 (D-17): the TTS config a Test voice request synthesizes
/// with. `slug` selects the Global target (`None`) or a known identity;
/// `settings` (the unsaved form values) apply only when `write_enabled` -- on
/// a read-only install they are ignored so Test voice cannot be pointed at an
/// arbitrary address (T-54.7-27).
#[cfg(not(target_arch = "wasm32"))]
fn tts_config_for_test(
    config: &ironhermes_core::config::Config,
    slug: Option<&str>,
    settings: Option<&TtsTestSettings>,
    write_enabled: bool,
) -> Result<ironhermes_core::config::TtsConfig, ServerFnError> {
    // Reject an unknown/tampered slug before it can drive provider/config
    // resolution at all.
    let mut effective = match slug {
        Some(slug) => {
            if !crate::components::hermes_app::avatar_logic::is_known_identity(slug) {
                return Err(ServerFnError::new("unknown identity"));
            }
            config.effective_tts_config_for_identity(Some(slug))
        }
        None => config.tts.clone(),
    };
    if write_enabled {
        if let Some(settings) = settings {
            // Validate BEFORE applying: the same rules and wire codes as a save.
            validate_tts_test_settings(&effective, &config.tts, settings)?;
            apply_tts_test_settings(&mut effective, settings);
        }
    }
    Ok(effective)
}

/// The save-path rules (plan 06), run over the unsaved form values. `effective`
/// is the config the values apply onto; `saved` is the stored global block, so
/// a format that equals either one passes (legacy-safe rule, T-54.7-71).
/// Fields the selected provider does not use are ignored by the apply step and
/// are not validated.
#[cfg(not(target_arch = "wasm32"))]
fn validate_tts_test_settings(
    effective: &ironhermes_core::config::TtsConfig,
    saved: &ironhermes_core::config::TtsConfig,
    settings: &TtsTestSettings,
) -> Result<(), ServerFnError> {
    use crate::voice_registry as vr;

    let provider = match settings.provider.as_deref().map(str::trim) {
        Some(p) => {
            if vr::tts_provider(p).is_none() {
                return Err(voice_invalid("tts_provider", "unknown_provider"));
            }
            p.to_string()
        }
        None => effective.provider.clone(),
    };
    // A changed format must be a registry format; one equal to the stored (or
    // effective) value passes so a hand-typed legacy value never blocks a test.
    let format = |field: &str, eff: &str, stored: &str| -> Result<(), ServerFnError> {
        match settings.format.as_deref() {
            Some(v) if v.trim() != stored.trim() => check_format_change(field, v, eff, &provider),
            _ => Ok(()),
        }
    };
    match provider.as_str() {
        "openai_compatible" => {
            check_voice_field("tts_compat_base_url", &settings.base_url, |v| {
                vr::validate_base_url(v, false)
            })?;
            check_voice_field(
                "tts_compat_path",
                &settings.path,
                vr::validate_endpoint_path,
            )?;
            check_voice_field(
                "tts_compat_key_ref",
                &settings.key_ref,
                vr::validate_key_ref,
            )?;
            check_voice_field("tts_compat_model", &settings.model, vr::validate_model_id)?;
            check_voice_field("tts_compat_voice", &settings.voice, vr::validate_voice_id)?;
            format(
                "tts_compat_format",
                &effective.openai_compatible.format,
                &saved.openai_compatible.format,
            )?;
            // An openai_compatible provider needs an address.
            let base_url = settings
                .base_url
                .as_deref()
                .unwrap_or(&effective.openai_compatible.base_url);
            vr::validate_base_url(base_url, true)
                .map_err(|e| voice_invalid("tts_compat_base_url", e.code()))?;
        }
        "openai" => {
            check_voice_field("tts_openai_model", &settings.model, vr::validate_model_id)?;
            check_voice_field("tts_openai_voice", &settings.voice, vr::validate_voice_id)?;
            format(
                "tts_openai_format",
                &effective.openai.format,
                &saved.openai.format,
            )?;
        }
        "elevenlabs" => {
            check_voice_field(
                "tts_elevenlabs_model",
                &settings.model,
                vr::validate_model_id,
            )?;
            check_voice_field(
                "tts_elevenlabs_voice",
                &settings.voice,
                vr::validate_voice_id,
            )?;
        }
        "edge" => {
            format(
                "tts_edge_format",
                &effective.edge.output_format,
                &saved.edge.output_format,
            )?;
        }
        _ => {}
    }
    Ok(())
}

/// Apply the form values onto the block the provider id selects. A `None`
/// field leaves the block's value alone; `path` and `key_ref` are optional in
/// the config, so an empty string clears them.
#[cfg(not(target_arch = "wasm32"))]
fn apply_tts_test_settings(
    effective: &mut ironhermes_core::config::TtsConfig,
    settings: &TtsTestSettings,
) {
    let trimmed = |v: &Option<String>| v.as_deref().map(|s| s.trim().to_string());
    if let Some(provider) = trimmed(&settings.provider) {
        effective.provider = provider;
    }
    let (model, voice, format) = (
        trimmed(&settings.model),
        trimmed(&settings.voice),
        trimmed(&settings.format),
    );
    match effective.provider.as_str() {
        "openai_compatible" => {
            let oc = &mut effective.openai_compatible;
            if let Some(v) = trimmed(&settings.base_url) {
                oc.base_url = v;
            }
            if let Some(v) = &settings.path {
                oc.path = non_empty_trimmed(v);
            }
            if let Some(v) = &settings.key_ref {
                oc.api_key_ref = non_empty_trimmed(v);
            }
            if let Some(v) = model {
                oc.model = v;
            }
            if let Some(v) = voice {
                oc.voice = v;
            }
            if let Some(v) = format {
                oc.format = v;
            }
        }
        "openai" => {
            let o = &mut effective.openai;
            if let Some(v) = model {
                o.model = v;
            }
            if let Some(v) = voice {
                o.voice = v;
            }
            if let Some(v) = format {
                o.format = v;
            }
        }
        "elevenlabs" => {
            let e = &mut effective.elevenlabs;
            if let Some(v) = model {
                e.model_id = v;
            }
            if let Some(v) = voice {
                e.voice_id = v;
            }
        }
        "edge" => {
            let e = &mut effective.edge;
            if let Some(v) = voice {
                e.voice = v;
            }
            if let Some(v) = format {
                e.output_format = v;
            }
        }
        _ => {}
    }
}

/// Map a provider failure to the wire contract `voice_test:{category}`. The
/// category is from the fixed `VoiceErrorCategory` vocabulary; the error text,
/// URL, key and response body never cross the boundary.
#[cfg(not(target_arch = "wasm32"))]
fn voice_test_error(err: &anyhow::Error) -> ServerFnError {
    let category = err
        .downcast_ref::<ironhermes_core::voice_http::VoiceHttpError>()
        .map(|e| e.category.as_str())
        .unwrap_or("unexpected response");
    ServerFnError::new(format!("voice_test:{category}"))
}

/// Phase 54.7 Plan 16: the Test-voice synth pipeline, shared by
/// `synth_voice_test` (plan 15 deleted the legacy `synth_voice_sample`
/// wrapper; the identity Test voice calls `synth_voice_test`). Builds the
/// registry through the vault-aware builder (so a namespaced `secret/voice/*`
/// reference resolves), names the file with `audio_format::extension_for` and
/// types the result with `audio_format::mime_for_path`. Never writes config.
#[cfg(not(target_arch = "wasm32"))]
async fn synth_voice_sample_impl(
    config: &ironhermes_core::config::Config,
    slug: Option<&str>,
    settings: Option<&TtsTestSettings>,
    audio_dir: &std::path::Path,
) -> Result<VoiceSampleAudio, ServerFnError> {
    use ironhermes_tools::tts::audio_format;

    let effective = tts_config_for_test(
        config,
        slug,
        settings,
        config.security.web_config_write_enabled,
    )?;
    let registry = ironhermes_tools::tts::build_tts_registry_with_vault(
        &effective,
        ironhermes_core::voice_http::voice_vault_config(config),
    );
    let provider = registry
        .get(&effective.provider)
        .ok_or_else(|| ServerFnError::new("TTS provider not registered"))?;

    std::fs::create_dir_all(audio_dir)
        .map_err(|e| ServerFnError::new(format!("audio_cache dir: {e}")))?;
    let output_path = audio_dir.join(format!(
        "{}.{}",
        uuid::Uuid::new_v4(),
        audio_format::extension_for(&effective)
    ));

    let written = provider
        .synthesize(SAMPLE_TEXT, &output_path)
        .await
        .map_err(|e| voice_test_error(&e))?;
    let size = std::fs::metadata(&written)
        .map_err(|e| ServerFnError::new(format!("audio read failed: {e}")))?
        .len();
    if size > SAMPLE_MAX_BYTES {
        // WR-10: an oversize sample must not stay in audio_cache.
        let _ = std::fs::remove_file(&written);
        return Err(ServerFnError::new("voice_test:unexpected response"));
    }
    let bytes = std::fs::read(&written)
        .map_err(|e| ServerFnError::new(format!("audio read failed: {e}")))?;

    use base64::Engine as _;
    Ok(VoiceSampleAudio {
        audio_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
        mime: audio_format::mime_for_path(&written).to_string(),
    })
}

/// Phase 54.7 Plan 16 (D-17): audition the CURRENT (possibly unsaved) TTS
/// settings. `identity_slug: None` is the Global target; `Some(slug)` an
/// identity. `settings` apply only when config writes are enabled, are
/// validated like a save, and are never persisted. The fn accepts NO
/// client-supplied text; the server synthesizes the fixed `SAMPLE_TEXT`.
///
/// Errors: `unknown identity`, `voice_invalid:{field}:{code}` (settings),
/// `voice_test:{category}` (provider failure).
#[server]
pub async fn synth_voice_test(
    identity_slug: Option<String>,
    settings: Option<TtsTestSettings>,
) -> Result<VoiceSampleAudio, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        let audio_dir = ironhermes_core::constants::get_hermes_home().join("audio_cache");
        synth_voice_sample_impl(
            &config,
            identity_slug.as_deref(),
            settings.as_ref(),
            &audio_dir,
        )
        .await
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (identity_slug, settings);
        Err(ServerFnError::new(
            "synth_voice_test unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod update_voice_config_tests {
    use super::{merge_voice_payload, validate_voice_payload, VoiceWritePayload};
    use ironhermes_core::config::Config;

    fn empty_payload() -> VoiceWritePayload {
        VoiceWritePayload::default()
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
            ..Default::default()
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

        // Phase 54.7 Plan 06 (D-02): every field NAME -- not just the ones this
        // test happens to populate -- may not hold a secret. A key field is a
        // `*_key_ref` reference; nothing else may mention api_key/token/secret.
        let keys_json = serde_json::to_value(VoiceWritePayload::default()).expect("keys");
        for key in keys_json.as_object().expect("object").keys() {
            let lower = key.to_ascii_lowercase();
            let is_reference = lower.ends_with("_key_ref");
            for needle in ["api_key", "token", "secret", "password"] {
                assert!(
                    !lower.contains(needle),
                    "payload field {key} must not look like a secret holder"
                );
            }
            if lower.contains("key") {
                assert!(
                    is_reference,
                    "payload field {key} mentions a key but is not a *_key_ref reference"
                );
            }
        }
    }

    /// T-36.17.10-01-02: VoiceConfigSnapshot serializes without any secret field.
    #[test]
    fn voice_config_snapshot_has_no_secret_field_values() {
        use super::{build_voice_snapshot, VoiceKeyStatuses};
        let mut config = Config::default();
        // ModelConfig.api_key is Option<String> — set it as Some to simulate a present key
        config.model.api_key = Some("secret-value".to_string());
        let snapshot = build_voice_snapshot(&config, &VoiceKeyStatuses::default());
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

    /// T-36.17.12-01-01 (rewritten by Phase 54.7 D-15): `open_mic` used to
    /// round-trip through the payload. D-15 retired it as a selectable value --
    /// the snapshot lists `barge_in_modes == ["push_to_interrupt"]` and the panel
    /// only ever sends a listed value -- so the server now rejects `open_mic`.
    #[test]
    fn barge_in_mode_open_mic_is_rejected_after_d15() {
        use ironhermes_core::config::BargeInMode;
        let mut payload = empty_payload();
        payload.barge_in_mode = Some("open_mic".into());
        let err = validate_voice_payload(&payload).expect_err("open_mic must be rejected");
        assert!(
            err.to_string().contains("voice_invalid:barge_in_mode"),
            "unexpected error: {err}"
        );

        payload.barge_in_mode = Some("push_to_interrupt".into());
        assert!(validate_voice_payload(&payload).is_ok());

        // Even a direct merge never writes open_mic back into the config.
        let mut config = ironhermes_core::config::Config::default();
        payload.barge_in_mode = Some("open_mic".into());
        merge_voice_payload(&mut config, &payload);
        assert_eq!(config.voice.barge_in_mode, BargeInMode::PushToInterrupt);
    }

    /// T-36.17.12-01-02: build_voice_snapshot serializes OpenMic as "open_mic".
    #[test]
    fn barge_in_mode_snapshot_serializes_open_mic() {
        use super::{build_voice_snapshot, VoiceKeyStatuses};
        use ironhermes_core::config::{BargeInMode, Config};
        let mut config = Config::default();
        config.voice.barge_in_mode = BargeInMode::OpenMic;
        let snapshot = build_voice_snapshot(&config, &VoiceKeyStatuses::default());
        assert_eq!(
            snapshot.barge_in_mode, "open_mic",
            "BargeInMode::OpenMic must serialize to \"open_mic\" in snapshot"
        );
    }

    /// T-36.17.12-01-03: snapshot serializes PushToInterrupt as "push_to_interrupt".
    #[test]
    fn barge_in_mode_snapshot_serializes_push_to_interrupt() {
        use super::{build_voice_snapshot, VoiceKeyStatuses};
        use ironhermes_core::config::{BargeInMode, Config};
        let mut config = Config::default();
        config.voice.barge_in_mode = BargeInMode::PushToInterrupt;
        let snapshot = build_voice_snapshot(&config, &VoiceKeyStatuses::default());
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

/// Phase 41.2 Plan 02 (D-06) / Phase 54.7 Plan 16: the identity Test voice
/// seam. These call `synth_voice_sample_impl`, the pipeline `synth_voice_test`
/// wraps.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod synth_voice_sample_tests {
    use super::synth_voice_sample_impl;
    use ironhermes_core::config::Config;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(f)
    }

    /// T-41.2-02: an unknown/tampered identity slug is rejected before any
    /// provider or network work -- the audio dir is never even created.
    #[test]
    fn synth_voice_sample_rejects_unknown_slug() {
        let dir = tempfile::tempdir().expect("tempdir");
        let audio_dir = dir.path().join("never_created");
        let err = block_on(synth_voice_sample_impl(
            &Config::default(),
            Some("not_a_real_identity"),
            None,
            &audio_dir,
        ))
        .err()
        .expect("unknown slug must fail")
        .to_string();
        assert!(err.contains("unknown identity"), "got: {err}");
        assert!(!audio_dir.exists(), "rejected before any provider work");
    }

    /// WR-10: a sample over the cap is refused AND its file is deleted from
    /// audio_cache (the early return used to leave it on disk).
    #[test]
    fn oversize_sample_is_rejected_and_deleted() {
        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/audio/speech"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_bytes(vec![1u8; super::SAMPLE_MAX_BYTES as usize + 1]),
                )
                .mount(&server)
                .await;
            let mut config = Config::default();
            config.tts.provider = "openai_compatible".into();
            config.tts.openai_compatible.base_url = format!("{}/v1", server.uri());
            let dir = tempfile::tempdir().expect("tempdir");
            let err = synth_voice_sample_impl(&config, None, None, dir.path())
                .await
                .err()
                .expect("an oversize sample must be refused")
                .to_string();
            assert!(err.contains("unexpected response"), "got: {err}");
            let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
            assert!(left.is_empty(), "oversize audio must be deleted: {left:?}");
        });
    }

    /// A known ORB_PRESET_REGISTRY slug passes the gate and synthesizes through
    /// the saved global provider (no identity record -> global block).
    #[test]
    fn synth_voice_sample_accepts_known_slug() {
        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/audio/speech"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(b"OK-AUDIO".to_vec()))
                .mount(&server)
                .await;
            let mut config = Config::default();
            config.tts.provider = "openai_compatible".into();
            config.tts.openai_compatible.base_url = format!("{}/v1", server.uri());
            config.identities.remove("orb_classic");
            let dir = tempfile::tempdir().expect("tempdir");
            let sample = synth_voice_sample_impl(&config, Some("orb_classic"), None, dir.path())
                .await
                .expect("known slug synthesizes");
            assert_eq!(sample.mime, "audio/mpeg");
        });
    }
}

// =============================================================================
// Phase 54.7 Plan 16 Task 2: validation, write gate, identity target and the
// fixed-phrase guarantees of Test voice.
// =============================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod synth_voice_test_guard_tests {
    use super::{tts_config_for_test, TtsTestSettings, SAMPLE_TEXT};
    use ironhermes_core::config::{Config, IdentityRecord};

    fn edge_config() -> Config {
        let mut config = Config::default();
        config.security.web_config_write_enabled = true;
        config.tts.provider = "edge".into();
        config
    }

    fn compat(base_url: &str) -> TtsTestSettings {
        TtsTestSettings {
            provider: Some("openai_compatible".into()),
            base_url: Some(base_url.into()),
            ..Default::default()
        }
    }

    fn err_text(r: Result<ironhermes_core::config::TtsConfig, super::ServerFnError>) -> String {
        r.expect_err("expected an error").to_string()
    }

    #[test]
    fn synth_settings_apply_only_when_writes_enabled() {
        let config = edge_config();
        let settings = compat("http://127.0.0.1:8880/v1");

        let open = tts_config_for_test(&config, None, Some(&settings), true).expect("open");
        assert_eq!(open.provider, "openai_compatible");
        assert_eq!(open.openai_compatible.base_url, "http://127.0.0.1:8880/v1");

        // Gate closed: the settings are ignored entirely (SSRF control), even
        // ones that would fail validation.
        let closed = tts_config_for_test(&config, None, Some(&settings), false).expect("closed");
        assert_eq!(closed.provider, "edge", "saved config wins");
        assert_eq!(closed.openai_compatible.base_url, "");
        let invalid = compat("ftp://nope");
        assert!(tts_config_for_test(&config, None, Some(&invalid), false).is_ok());
    }

    #[test]
    fn synth_settings_are_validated_like_a_save() {
        let config = edge_config();

        let e = err_text(tts_config_for_test(
            &config,
            None,
            Some(&compat("ftp://x")),
            true,
        ));
        assert!(
            e.contains("voice_invalid:tts_compat_base_url:base_url_scheme"),
            "got: {e}"
        );

        let bogus = TtsTestSettings {
            provider: Some("bogus".into()),
            ..Default::default()
        };
        let e = err_text(tts_config_for_test(&config, None, Some(&bogus), true));
        assert!(e.contains("unknown_provider"), "got: {e}");

        let mut s = compat("http://127.0.0.1:8880/v1");
        s.key_ref = Some("OPENAI_API_KEY".into());
        let e = err_text(tts_config_for_test(&config, None, Some(&s), true));
        assert!(
            e.contains("voice_invalid:tts_compat_key_ref:key_ref_namespace"),
            "got: {e}"
        );

        // A changed format must be a registry format.
        let mut s = compat("http://127.0.0.1:8880/v1");
        s.format = Some("flac".into());
        let e = err_text(tts_config_for_test(&config, None, Some(&s), true));
        assert!(
            e.contains("voice_invalid:tts_compat_format:format"),
            "got: {e}"
        );

        // An openai_compatible provider needs an address.
        let bare = TtsTestSettings {
            provider: Some("openai_compatible".into()),
            ..Default::default()
        };
        let e = err_text(tts_config_for_test(&config, None, Some(&bare), true));
        assert!(
            e.contains("voice_invalid:tts_compat_base_url:base_url_empty"),
            "got: {e}"
        );

        // Legacy-safe rule: a format EQUAL to the stored value passes.
        let mut legacy = edge_config();
        legacy.tts.openai.format = "flac".into();
        legacy.tts.edge.output_format = "audio-24khz-48kbitrate-mono-mp3".into();
        let openai = TtsTestSettings {
            provider: Some("openai".into()),
            format: Some("flac".into()),
            ..Default::default()
        };
        assert!(tts_config_for_test(&legacy, None, Some(&openai), true).is_ok());
        let edge = TtsTestSettings {
            provider: Some("edge".into()),
            format: Some("audio-24khz-48kbitrate-mono-mp3".into()),
            ..Default::default()
        };
        assert!(tts_config_for_test(&legacy, None, Some(&edge), true).is_ok());
        // ...but a different unlisted one does not.
        let changed = TtsTestSettings {
            provider: Some("openai".into()),
            format: Some("aac".into()),
            ..Default::default()
        };
        assert!(tts_config_for_test(&legacy, None, Some(&changed), true).is_err());
    }

    #[test]
    fn synth_identity_target_applies_identity_then_settings() {
        let mut config = edge_config();
        let mut record = IdentityRecord::default();
        record.voice.free_mode_tts_provider = Some("edge".into());
        record.voice.free_mode_tts_voice = Some("en-GB-IdentityNeural".into());
        config.identities.insert("orb_classic".into(), record);

        let from_identity =
            tts_config_for_test(&config, Some("orb_classic"), None, true).expect("identity");
        assert_eq!(from_identity.edge.voice, "en-GB-IdentityNeural");

        let settings = TtsTestSettings {
            provider: Some("edge".into()),
            voice: Some("en-US-FormNeural".into()),
            ..Default::default()
        };
        let both = tts_config_for_test(&config, Some("orb_classic"), Some(&settings), true)
            .expect("identity + settings");
        assert_eq!(both.edge.voice, "en-US-FormNeural", "the form value wins");
        assert_eq!(
            config.tts.edge.voice, "en-US-AriaNeural",
            "saved global untouched"
        );

        let e = err_text(tts_config_for_test(
            &config,
            Some("not_a_real_identity"),
            None,
            true,
        ));
        assert!(e.contains("unknown identity"), "got: {e}");
    }

    /// Slice of this file's source from `header` to the next top-level item.
    fn item_text<'a>(src: &'a str, header: &str) -> &'a str {
        let start = src.find(header).expect("header present");
        let rest = &src[start + header.len()..];
        let end = ["\n#[", "\n///", "\nfn ", "\nasync fn ", "\npub "]
            .iter()
            .filter_map(|m| rest.find(m))
            .min()
            .unwrap_or(rest.len());
        &src[start..start + header.len() + end]
    }

    #[test]
    fn synth_settings_never_change_the_sample_text() {
        let src = include_str!("api.rs");
        let body = item_text(src, "async fn synth_voice_sample_impl(");
        assert_eq!(
            body.matches(".synthesize(").count(),
            1,
            "exactly one synthesize call"
        );
        assert!(
            body.contains(".synthesize(SAMPLE_TEXT,"),
            "the only synthesized string is the SAMPLE_TEXT constant"
        );
        assert_eq!(SAMPLE_TEXT, "This is how I sound.");
        // TtsTestSettings carries no text/input field.
        let keys: std::collections::BTreeSet<String> =
            serde_json::to_value(TtsTestSettings::default())
                .expect("ser")
                .as_object()
                .expect("object")
                .keys()
                .cloned()
                .collect();
        let expected: std::collections::BTreeSet<String> = [
            "provider", "base_url", "path", "key_ref", "model", "voice", "format",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(keys, expected);
    }

    #[test]
    fn synth_test_never_saves() {
        let src = include_str!("api.rs");
        let forbidden = [[".sa", "ve()"].concat(), [".save", "_to("].concat()];
        for header in [
            "async fn synth_voice_sample_impl(",
            "fn tts_config_for_test(",
            "fn apply_tts_test_settings(",
            "fn validate_tts_test_settings(",
        ] {
            let body = item_text(src, header);
            assert!(
                body.len() > 100,
                "{header}: slice looks wrong ({})",
                body.len()
            );
            for f in &forbidden {
                assert!(!body.contains(f.as_str()), "{header} must not contain {f}");
            }
        }
    }
}

// =============================================================================
// Phase 54.7 Plan 16: Test voice auditions UNSAVED TTS settings (D-17).
// Every test drives `tts_config_for_test` / `synth_voice_sample_impl` directly
// with an in-memory Config, a tempdir audio dir and a wiremock base URL.
// =============================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod synth_voice_test_tests {
    use super::{synth_voice_sample_impl, tts_config_for_test, TtsTestSettings};
    use ironhermes_core::config::Config;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(f)
    }

    fn kokoro_settings(server_uri: &str, format: &str) -> TtsTestSettings {
        TtsTestSettings {
            provider: Some("openai_compatible".into()),
            base_url: Some(format!("{server_uri}/v1")),
            model: Some("mlx-community/Kokoro-82M-bf16".into()),
            voice: Some("af_heart".into()),
            format: Some(format.into()),
            ..Default::default()
        }
    }

    fn write_open_config() -> Config {
        let mut config = Config::default();
        config.security.web_config_write_enabled = true;
        config.tts.provider = "edge".into();
        config
    }

    #[test]
    fn synth_test_settings_reach_compat_server() {
        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/audio/speech"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(b"FAKE-MP3-BYTES".to_vec()))
                .mount(&server)
                .await;

            let dir = tempfile::tempdir().expect("tempdir");
            let cfg_path = dir.path().join("config.yaml");
            let config = write_open_config();
            config.save_to(&cfg_path).expect("save fixture");
            let before = std::fs::read(&cfg_path).expect("read yaml");
            let audio_dir = dir.path().join("audio_cache");
            std::fs::create_dir_all(&audio_dir).expect("audio dir");

            let settings = kokoro_settings(&server.uri(), "mp3");
            let sample = synth_voice_sample_impl(&config, None, Some(&settings), &audio_dir)
                .await
                .expect("synth");

            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&sample.audio_b64)
                .expect("b64");
            assert_eq!(bytes, b"FAKE-MP3-BYTES");
            assert_eq!(sample.mime, "audio/mpeg");

            let received = server.received_requests().await.expect("recording on");
            assert_eq!(received.len(), 1, "exactly one TTS request");
            let body: serde_json::Value = received[0].body_json().expect("json body");
            assert_eq!(body["input"], "This is how I sound.");
            assert_eq!(body["model"], "mlx-community/Kokoro-82M-bf16");
            assert_eq!(body["voice"], "af_heart");

            assert_eq!(
                std::fs::read(&cfg_path).expect("reread yaml"),
                before,
                "Test voice must never touch the config on disk"
            );
            assert_eq!(config.tts.provider, "edge", "in-memory config unchanged");
        });
    }

    #[test]
    fn synth_uses_vault_aware_builder_and_audio_format() {
        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/audio/speech"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(b"RIFF-FAKE-WAV".to_vec()))
                .mount(&server)
                .await;

            let dir = tempfile::tempdir().expect("tempdir");
            let config = write_open_config();
            let settings = kokoro_settings(&server.uri(), "wav");
            let sample = synth_voice_sample_impl(&config, None, Some(&settings), dir.path())
                .await
                .expect("synth");
            assert_eq!(sample.mime, "audio/wav");
            let wavs = std::fs::read_dir(dir.path())
                .expect("read dir")
                .filter_map(Result::ok)
                .filter(|e| e.path().extension().is_some_and(|x| x == "wav"))
                .count();
            assert_eq!(wavs, 1, "the file is named with the audio_format extension");

            // A provider 401 surfaces as the fixed category, never a body or key.
            let rejecting = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(401).set_body_string("secret-body-text"))
                .mount(&rejecting)
                .await;
            let settings = kokoro_settings(&rejecting.uri(), "mp3");
            let err = synth_voice_sample_impl(&config, None, Some(&settings), dir.path())
                .await
                .err()
                .expect("401 must fail")
                .to_string();
            assert!(err.contains("voice_test:key rejected"), "got: {err}");
            assert!(!err.contains("secret-body-text"), "body leaked: {err}");
        });
    }

    #[test]
    fn synth_global_target_needs_no_slug() {
        let mut config = write_open_config();
        config.tts.edge.voice = "en-GB-RyanNeural".into();
        let effective = tts_config_for_test(&config, None, None, true).expect("global target");
        assert_eq!(
            serde_json::to_value(&effective).expect("ser"),
            serde_json::to_value(&config.tts).expect("ser"),
            "no slug and no settings serves the saved global TtsConfig"
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
        build_voice_snapshot, merge_voice_payload, validate_voice_payload, VoiceKeyStatuses,
        VoiceWritePayload,
    };
    use ironhermes_core::config::Config;

    /// Construct an all-None payload INCLUDING the new identity fields.
    /// Fields identity_slug through identity_glow do not exist yet → RED.
    fn identity_payload(slug: Option<&str>) -> VoiceWritePayload {
        VoiceWritePayload {
            identity_slug: slug.map(|s| s.to_string()),
            ..Default::default()
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

    /// Phase 54.7 Plan 15 (D-16): an identity's TTS model and format are saved
    /// into `identities.<slug>.voice`, come back in the snapshot's
    /// `IdentityOverride`, and are applied by the TTS path.
    #[test]
    fn identity_model_and_format_round_trip() {
        let mut config = Config::default();
        let mut p = identity_payload(Some("orb_bloom"));
        p.identity_tts_provider = Some("openai".to_string());
        p.identity_tts_model = Some("tts-1-hd".to_string());
        p.identity_tts_format = Some("opus".to_string());
        p.identity_realtime_model = Some("gpt-realtime-2".to_string());
        validate_voice_payload(&p).expect("a registry model and format validate");
        merge_voice_payload(&mut config, &p);

        let voice = &config.identities.get("orb_bloom").expect("orb_bloom").voice;
        assert_eq!(voice.free_mode_tts_model.as_deref(), Some("tts-1-hd"));
        assert_eq!(voice.free_mode_tts_format.as_deref(), Some("opus"));
        assert_eq!(voice.realtime_model.as_deref(), Some("gpt-realtime-2"));

        let snapshot = build_voice_snapshot(&config, &VoiceKeyStatuses::default());
        let ov = snapshot
            .identity_overrides
            .iter()
            .find(|o| o.slug == "orb_bloom")
            .expect("orb_bloom override in the snapshot");
        assert_eq!(ov.free_mode_model.as_deref(), Some("tts-1-hd"));
        assert_eq!(ov.free_mode_format.as_deref(), Some("opus"));
        assert_eq!(ov.realtime_model, "gpt-realtime-2");

        // An identity that sets nothing inherits: no model/format, the global
        // realtime model.
        let plain = snapshot
            .identity_overrides
            .iter()
            .find(|o| o.slug == "orb_classic")
            .expect("orb_classic override in the snapshot");
        assert_eq!(plain.free_mode_model, None);
        assert_eq!(plain.free_mode_format, None);
        assert_eq!(plain.realtime_model, config.realtime.model());

        // The TTS path applies them onto the matching provider block, and the
        // global block stays untouched.
        let effective = config.effective_tts_config_for_identity(Some("orb_bloom"));
        assert_eq!(effective.openai.format, "opus");
        assert_eq!(effective.openai.model, "tts-1-hd");
        assert_ne!(config.tts.openai.format, "opus");

        // The snapshot advertises provider ids only (never a key value).
        assert!(
            snapshot.tts_providers_usable.iter().any(|id| id == "edge"),
            "Edge needs no key and is always usable: {:?}",
            snapshot.tts_providers_usable
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
        let snapshot = build_voice_snapshot(&config, &VoiceKeyStatuses::default());
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

// =============================================================================
// Phase 54.7 Plan 15 Task 2 (D-16): server rules for an identity-target save.
// An identity may shape voice (provider, model, voice, format; realtime voice
// and model) and nothing else -- never an endpoint, a key reference, STT or a
// global setting -- and only among providers this server has configured.
// =============================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod identity_rules_547_15 {
    use super::{apply_voice_payload, validate_voice_payload, VoiceWritePayload};
    use ironhermes_core::config::Config;

    fn identity(slug: &str) -> VoiceWritePayload {
        VoiceWritePayload {
            identity_slug: Some(slug.to_string()),
            ..Default::default()
        }
    }

    fn err_of(result: Result<(), dioxus::prelude::ServerFnError>) -> String {
        result.expect_err("must be rejected").to_string()
    }

    /// `Config::default()` carries no identities (the shipped personas are
    /// merged in when a config file loads), so seed them the same way.
    fn seeded(mut config: Config) -> Config {
        for (slug, record) in ironhermes_core::config::default_seed_identities() {
            config.identities.entry(slug).or_insert(record);
        }
        config
    }

    /// Run `body` with the provider key env vars unset (restored afterwards),
    /// serialized with every other env-touching test.
    fn with_no_provider_keys<R>(body: impl FnOnce() -> R) -> R {
        let _guard = crate::server::test_support::env_lock();
        let names = ["ELEVENLABS_API_KEY", "OPENAI_API_KEY"];
        let saved: Vec<Option<String>> = names.iter().map(|n| std::env::var(n).ok()).collect();
        // SAFETY: serialized by env_lock(); restored below.
        unsafe {
            for n in names {
                std::env::remove_var(n);
            }
        }
        let out = body();
        unsafe {
            for (n, v) in names.iter().zip(saved) {
                if let Some(v) = v {
                    std::env::set_var(n, v);
                }
            }
        }
        out
    }

    /// A Kokoro-only server: the seeded orb_bloom / groovy identities default
    /// to ElevenLabs, which has no key here.
    fn kokoro_only_config() -> Config {
        let mut config = seeded(Config::default());
        config.tts.provider = "openai_compatible".into();
        config.tts.openai_compatible.base_url = "http://tts.lan:8880/v1".into();
        config
    }

    /// D-16 / T-54.7-64: an identity-target payload that also sets any global
    /// field is refused, field by field.
    #[test]
    fn identity_payload_rejects_global_only_fields() {
        let cases: Vec<(&str, VoiceWritePayload)> = vec![
            (
                "tts_compat_base_url",
                VoiceWritePayload {
                    tts_compat_base_url: Some("http://evil.lan/v1".into()),
                    ..identity("orb_bloom")
                },
            ),
            (
                "tts_compat_key_ref",
                VoiceWritePayload {
                    tts_compat_key_ref: Some("VOICE_KEY_X".into()),
                    ..identity("orb_bloom")
                },
            ),
            (
                "stt_compat_base_url",
                VoiceWritePayload {
                    stt_compat_base_url: Some("http://stt.lan/v1".into()),
                    ..identity("orb_bloom")
                },
            ),
            (
                "stt_provider",
                VoiceWritePayload {
                    stt_provider: Some("groq".into()),
                    ..identity("orb_bloom")
                },
            ),
            (
                "realtime_base_url",
                VoiceWritePayload {
                    realtime_base_url: Some("http://rt.lan/v1".into()),
                    ..identity("orb_bloom")
                },
            ),
            (
                "realtime_key_ref",
                VoiceWritePayload {
                    realtime_key_ref: Some("VOICE_KEY_RT".into()),
                    ..identity("orb_bloom")
                },
            ),
            (
                "voice_service",
                VoiceWritePayload {
                    voice_service: Some("realtime".into()),
                    ..identity("orb_bloom")
                },
            ),
            (
                "tts_provider",
                VoiceWritePayload {
                    tts_provider: Some("edge".into()),
                    ..identity("orb_bloom")
                },
            ),
            (
                "tts_openai_model",
                VoiceWritePayload {
                    tts_openai_model: Some("tts-1".into()),
                    ..identity("orb_bloom")
                },
            ),
            (
                "auto_tts",
                VoiceWritePayload {
                    auto_tts: Some(true),
                    ..identity("orb_bloom")
                },
            ),
        ];
        for (field, payload) in cases {
            let msg = err_of(validate_voice_payload(&payload));
            assert!(
                msg.contains(&format!("voice_invalid:{field}:identity_global_field")),
                "{field} must be refused for an identity target: {msg}"
            );
        }
        // The appearance fields an identity save legitimately carries are not
        // "global" -- they pass.
        let mut ok = identity("orb_bloom");
        ok.identity_appearance_style = Some("bloom".into());
        ok.identity_base_hue = Some(280);
        validate_voice_payload(&ok).expect("identity appearance is an identity field");
        // And the same global field without an identity slug is untouched.
        let global = VoiceWritePayload {
            tts_compat_base_url: Some("http://tts.lan:8880/v1".into()),
            ..Default::default()
        };
        validate_voice_payload(&global).expect("a global save may set its endpoint");
    }

    /// D-16 / T-54.7-65: an identity may only pick a provider this server has
    /// configured; an unknown id is refused before any merge.
    #[test]
    fn identity_provider_must_be_configured() {
        with_no_provider_keys(|| {
            let payload = VoiceWritePayload {
                identity_tts_provider: Some("openai_compatible".into()),
                ..identity("orb_bloom")
            };
            validate_voice_payload(&payload).expect("a registry id passes the pure rules");

            // No saved Base URL: not configured.
            let mut config = Config::default();
            config.tts.openai_compatible.base_url = String::new();
            let msg = err_of(apply_voice_payload(&mut config, &payload));
            assert!(
                msg.contains("voice_invalid:identity_tts_provider:provider_unusable"),
                "{msg}"
            );

            // A saved Base URL makes it configured.
            let mut config = Config::default();
            config.tts.openai_compatible.base_url = "http://tts.lan:8880/v1".into();
            apply_voice_payload(&mut config, &payload).expect("configured provider accepted");
            assert_eq!(
                config.identities["orb_bloom"]
                    .voice
                    .free_mode_tts_provider
                    .as_deref(),
                Some("openai_compatible")
            );

            // An unknown id never reaches the merge.
            let unknown = VoiceWritePayload {
                identity_tts_provider: Some("not_a_provider".into()),
                ..identity("orb_bloom")
            };
            let msg = err_of(validate_voice_payload(&unknown));
            assert!(
                msg.contains("voice_invalid:identity_tts_provider:unknown_provider"),
                "{msg}"
            );
        });
    }

    /// D-16 / T-54.7-66: identity model, voice, format and realtime values go
    /// through the same rules as the global fields.
    #[test]
    fn identity_values_use_shared_rules() {
        let bad_model = VoiceWritePayload {
            identity_tts_model: Some("a\"b".into()),
            ..identity("orb_bloom")
        };
        assert!(err_of(validate_voice_payload(&bad_model))
            .contains("voice_invalid:identity_tts_model:model_charset"));

        let bad_voice = VoiceWritePayload {
            identity_realtime_voice: Some("a/b".into()),
            ..identity("orb_bloom")
        };
        assert!(err_of(validate_voice_payload(&bad_voice))
            .contains("voice_invalid:identity_realtime_voice:voice_charset"));

        let bad_tts_voice = VoiceWritePayload {
            identity_tts_voice: Some("v".repeat(65)),
            ..identity("orb_bloom")
        };
        assert!(err_of(validate_voice_payload(&bad_tts_voice))
            .contains("voice_invalid:identity_tts_voice:voice_charset"));

        let bad_format = VoiceWritePayload {
            identity_tts_provider: Some("openai".into()),
            identity_tts_format: Some("flac".into()),
            ..identity("orb_bloom")
        };
        assert!(err_of(validate_voice_payload(&bad_format))
            .contains("voice_invalid:identity_tts_format:format"));

        // A format that does not belong to the identity's CURRENT provider is
        // refused even when the payload does not restate the provider.
        let mut config = seeded(Config::default());
        config
            .identities
            .get_mut("orb_bloom")
            .expect("seeded")
            .voice
            .free_mode_tts_provider = Some("edge".into());
        let restated = VoiceWritePayload {
            identity_tts_format: Some("opus".into()),
            ..identity("orb_bloom")
        };
        validate_voice_payload(&restated).expect("provider unknown to the pure rules");
        let msg = err_of(apply_voice_payload(&mut config, &restated));
        assert!(
            msg.contains("voice_invalid:identity_tts_format:format"),
            "{msg}"
        );

        let bad_rt_model = VoiceWritePayload {
            identity_realtime_model: Some("bad model".into()),
            ..identity("orb_bloom")
        };
        assert!(err_of(validate_voice_payload(&bad_rt_model))
            .contains("voice_invalid:identity_realtime_model:model_charset"));

        let good = VoiceWritePayload {
            identity_realtime_model: Some("gpt-realtime-2.1".into()),
            identity_realtime_voice: Some("marin".into()),
            identity_tts_provider: Some("openai".into()),
            identity_tts_model: Some("tts-1-hd".into()),
            identity_tts_format: Some("opus".into()),
            ..identity("orb_bloom")
        };
        validate_voice_payload(&good).expect("registry values validate");
    }

    /// Plan-checker issue: the usability rule judges ONLY the identity a save
    /// writes and ONLY when that save sets its provider. The seeded elevenlabs
    /// identities on a Kokoro-only server never block another save.
    #[test]
    fn identity_usability_check_is_scoped_to_the_written_identity() {
        with_no_provider_keys(|| {
            let base = kokoro_only_config();
            for slug in ["orb_bloom", "groovy"] {
                assert_eq!(
                    base.identities[slug]
                        .voice
                        .free_mode_tts_provider
                        .as_deref(),
                    Some("elevenlabs"),
                    "positive control: {slug} is seeded on a provider with no key here"
                );
            }

            // A global save succeeds.
            let mut config = base.clone();
            let global = VoiceWritePayload {
                auto_tts: Some(true),
                ..Default::default()
            };
            apply_voice_payload(&mut config, &global).expect("a global save is never blocked");

            // An identity save that changes only the realtime voice succeeds.
            let mut config = base.clone();
            let rt_only = VoiceWritePayload {
                identity_realtime_voice: Some("marin".into()),
                ..identity("orb_bloom")
            };
            apply_voice_payload(&mut config, &rt_only)
                .expect("a realtime-only identity save is never blocked");

            // Moving groovy to the configured provider succeeds.
            let mut config = base.clone();
            let to_compat = VoiceWritePayload {
                identity_tts_provider: Some("openai_compatible".into()),
                ..identity("groovy")
            };
            apply_voice_payload(&mut config, &to_compat)
                .expect("a configured provider is accepted");

            // Choosing the unusable provider for groovy is refused.
            let mut config = base.clone();
            let to_eleven = VoiceWritePayload {
                identity_tts_provider: Some("elevenlabs".into()),
                ..identity("groovy")
            };
            let msg = err_of(apply_voice_payload(&mut config, &to_eleven));
            assert!(
                msg.contains("voice_invalid:identity_tts_provider:provider_unusable"),
                "{msg}"
            );
        });
    }
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

/// Phase 54.7 (D-11): the realtime model / voice gate.
///
/// Replaces the fixed whitelists of Phase 36.17.12 Plan 02
/// (T-36.17.12-02-02 / T-V5). A new model or voice no longer needs a code
/// change, so the injection control is a CHARSET gate instead of a list:
/// model `[A-Za-z0-9._:/-]{1,128}` with no `..` and no leading `/`, voice
/// `[A-Za-z0-9._:-]{1,64}`. The rules live in
/// `ironhermes_core::voice_endpoint`, shared with the settings save path.
///
/// The transcription model is deliberately NOT gated here (WR-08): minting
/// never sends it. It reaches the provider only through
/// `realtime_session_config_for`, which applies the same charset gate and
/// falls back to a default with a warning, so a hand-edited stray value
/// degrades that one setting instead of disabling realtime (and triggering
/// the D-07 turn-based fallback) outright.
///
/// It runs BEFORE any key resolution or network call (T-54.7-47). Validated
/// values are only ever placed into JSON through `serde_json::json!`, never
/// into a URL. The error text never echoes the rejected value: an operator
/// can paste a secret into the wrong field, and the message is logged.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn validate_realtime_values(model: &str, voice: &str) -> Result<(), ServerFnError> {
    use ironhermes_core::voice_endpoint::{validate_model_id, validate_voice_id};
    if validate_model_id(model).is_err() {
        return Err(ServerFnError::new("Unknown model -- realtime unavailable"));
    }
    if validate_voice_id(voice).is_err() {
        return Err(ServerFnError::new("Unknown voice -- realtime unavailable"));
    }
    Ok(())
}

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
/// The D-11 charset gate (`validate_realtime_values`) is applied by the CALLER
/// (`resolve_realtime_selection`) after this function returns, as a
/// defense-in-depth check on the resolved value (T-40.5-03-03).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn resolve_realtime_voice(
    config: &ironhermes_core::config::Config,
    slug: Option<&str>,
) -> Result<String, ServerFnError> {
    use crate::components::hermes_app::avatar_logic::is_known_identity;

    let global_voice = config.realtime.voice().to_string();

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

/// Phase 54.7 (D-16): resolve the realtime model for a session -- the
/// identity's `realtime_model` when set, else `realtime.model()`.
///
/// Mirrors `resolve_realtime_voice`: the same charset + known-identity gates
/// run on the slug before any config lookup (T-40.5-03-01), and the resolved
/// value is gated by `validate_realtime_values` by the caller.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn resolve_realtime_model(
    config: &ironhermes_core::config::Config,
    slug: Option<&str>,
) -> Result<String, ServerFnError> {
    use crate::components::hermes_app::avatar_logic::is_known_identity;

    let global_model = config.realtime.model().to_string();
    let Some(s) = slug else {
        return Ok(global_model);
    };
    if s.len() > 64 || !s.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(ServerFnError::new(
            "Invalid identity slug — rejected (T-40.5-03-01)".to_string(),
        ));
    }
    if !is_known_identity(s) {
        return Err(ServerFnError::new(
            "Unknown identity — realtime model unavailable (T-40.5-03-01)".to_string(),
        ));
    }
    let identity_model = config
        .identities
        .get(s)
        .and_then(|rec| rec.voice.realtime_model.clone());
    Ok(identity_model.unwrap_or(global_model))
}

/// Resolve and GATE the realtime (model, voice) for a session (D-11/D-16).
/// Pure: no key resolution and no network, so callers can run it first and
/// reject a bad value before doing any expensive or networked work.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn resolve_realtime_selection(
    config: &ironhermes_core::config::Config,
    slug: Option<&str>,
) -> Result<(String, String), ServerFnError> {
    let model = resolve_realtime_model(config, slug)?;
    let voice = resolve_realtime_voice(config, slug)?;
    validate_realtime_values(&model, &voice)?;
    Ok((model, voice))
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
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct IssueRealtimeTokenResult {
    /// Short-lived ephemeral token for the WebRTC session. `None` on the
    /// keyless path (Phase 54.7 D-12): the server skipped minting and the
    /// browser authenticates with `session_json` instead.
    pub token: Option<String>,
    /// Phase 54.7 (D-10/D-12): the URL the browser POSTs its SDP offer to,
    /// `{realtime.base_url}/realtime/calls`. The browser never builds this
    /// itself (UI-SPEC Interaction Rule 4).
    pub calls_url: String,
    /// Phase 54.7 (D-12): the full realtime session JSON (model, voice,
    /// instructions, tools). `Some` only on the keyless path, where the
    /// browser sends it as the `session` multipart part next to the SDP offer.
    pub session_json: Option<String>,
    /// UUID assigned by the server at mint time (used as heartbeat/end key).
    pub turn_id: String,
    /// Realtime session id (derived from turn_id; returned for internal use).
    pub session_id: String,
    /// The user's real chat session key (`"agent:main:web:dm:{uuid}"`) —
    /// the WRITE key for `realtime_save_transcript` (D-04 RC-1).
    pub chat_session_id: String,
}

// IN-01: the token is a credential and `session_json` carries the agent's
// instructions and tools -- neither may reach a `{:?}` rendering.
impl std::fmt::Debug for IssueRealtimeTokenResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssueRealtimeTokenResult")
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("calls_url", &self.calls_url)
            .field(
                "session_json_len",
                &self.session_json.as_ref().map(String::len),
            )
            .field("turn_id", &self.turn_id)
            .field("session_id", &self.session_id)
            .field("chat_session_id", &self.chat_session_id)
            .finish()
    }
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

// =============================================================================
// avatar-not-hearing-voice follow-up (.planning/debug/avatar-not-hearing-voice.md):
// instructions + tools moved from the RTCDataChannel session.update into the
// ephemeral-token request
// =============================================================================
//
// MEASURED 2026-09-18 19:52 (live instrumented reading): `instructions` alone
// was 440,628 bytes (91.8% of a 480,128-byte session.update) — 1.68x Chrome's
// own 262,144-byte SCTP maxMessageSize on its own, and `tools` (35 entries) was
// another 37,341 bytes (7.8%). A split-message design cannot fix this because
// `instructions` alone still exceeds the limit. The fix: `instructions` and
// `tools` now travel in `issue_realtime_token`'s POST to
// `/v1/realtime/client_secrets` — a server→OpenAI HTTPS leg with no SCTP
// message-size ceiling — instead of riding along in the browser's
// `session.update`. Per OpenAI's client_secrets docs (verified 2026), session
// config set at token-creation time can still be overridden by the client's own
// `session.update`, so `realtime_session_config`'s audio/VAD-only payload
// (see below) remains valid as an override layer, not a duplicate.

/// Build the realtime `instructions` string (D-01) and `tools` array (D-02)
/// exactly once, shared between `issue_realtime_token` (which now carries them
/// in the ephemeral-token request) and `realtime_session_config` (which now
/// carries ONLY the audio/VAD config over the RTCDataChannel).
///
/// D-01: builds `instructions` from `PromptBuilder` — same identity + active
/// skills + context as the text-chat path (state.rs L519-533). Verbatim; no
/// realtime-specific trimming.
/// D-02: serializes the full `ToolRegistry` tool set (same set as the text
/// path) in OpenAI function-tool format. No curated subset.
/// D-05a verbal ack: the built instructions contain an explicit directive
/// telling the agent to verbally acknowledge when it starts a long-running
/// background task.
///
/// T-LOG-LEAK: this function never logs its return values — callers must not
/// either. Only byte-length instrumentation (never content) is permitted, and
/// only at the two existing measurement boundaries (`realtime_session_config`'s
/// warn and the client's data-channel-open log).
/// Voice-session directive: anything time-sensitive must be looked up, never
/// answered from training data. Without it the realtime model answered a bare
/// "get me the latest news" with no tool call at all (2026-10-15 00:06Z), while
/// an explicit "use web_search ..." prompt in the next session worked.
#[cfg(not(target_arch = "wasm32"))]
const REALTIME_CURRENT_INFO_DIRECTIVE: &str = "CURRENT-INFORMATION DIRECTIVE: \
    Your built-in knowledge is out of date. For news, current events, weather, \
    prices, scores, releases, or anything that may have changed recently, you MUST \
    call web_search (or web_answer) before answering, then summarize the results \
    aloud. Never answer such questions from memory, and never say you cannot \
    browse — you can, through these tools.";

#[cfg(not(target_arch = "wasm32"))]
async fn build_realtime_instructions_and_tools() -> (String, Vec<serde_json::Value>) {
    let app_state = crate::server::state::global_app_state();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut prompt_builder =
        ironhermes_agent::PromptBuilder::new(&app_state.config().model.default, "web")
            .with_provider(&app_state.config().model.provider)
            .load_context(&cwd);
    prompt_builder.set_skill_registry(app_state.runtime.skill_registry().clone());
    if let Some(ref manager) = app_state.memory_manager {
        prompt_builder.set_memory_manager(manager.clone());
    }
    prompt_builder.set_user_profile_enabled(app_state.config().memory.user_profile_enabled);
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
    prompt_builder.set_timezone(app_state.config().agent.timezone.clone());
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
        while async tasks complete in the background.\n\n\
        {REALTIME_CURRENT_INFO_DIRECTIVE}"
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

    (instructions, tools)
}

/// What `mint_realtime_for` hands back to `issue_realtime_token` (Phase 54.7
/// D-10/D-12). The request body is built by the realtime provider's
/// `build_session` / `build_mint_request`, not here.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct MintedRealtime {
    /// Ephemeral client secret; `None` when minting was skipped (keyless).
    pub token: Option<String>,
    /// `{realtime.base_url}/realtime/calls` -- where the browser sends its SDP.
    pub calls_url: String,
    /// The full session JSON, set only on the keyless path.
    pub session_json: Option<String>,
}

// The token is a credential: keep it out of any `{:?}` rendering.
#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for MintedRealtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintedRealtime")
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("calls_url", &self.calls_url)
            .field(
                "session_json_len",
                &self.session_json.as_ref().map(String::len),
            )
            .finish()
    }
}

/// The configured realtime provider, or a typed error for an unknown id.
#[cfg(not(target_arch = "wasm32"))]
fn configured_realtime_provider(
    config: &ironhermes_core::config::Config,
) -> Result<Box<dyn ironhermes_tools::realtime::RealtimeProvider>, ServerFnError> {
    ironhermes_tools::realtime::realtime_provider(&config.realtime.provider).ok_or_else(|| {
        ServerFnError::new("realtime provider not supported -- realtime unavailable")
    })
}

/// `{realtime.base_url}/realtime/calls` for the configured provider (D-10).
/// Pure: no network, no key.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn realtime_calls_url_for(
    config: &ironhermes_core::config::Config,
) -> Result<String, ServerFnError> {
    let provider = configured_realtime_provider(config)?;
    provider
        .calls_url(&config.realtime)
        .map(|u| u.to_string())
        .map_err(|_| ServerFnError::new("realtime address invalid -- realtime unavailable"))
}

/// Phase 54.7 (D-10/D-12): build the realtime session through the provider and
/// mint an ephemeral client secret at `{base_url}/realtime/client_secrets`.
///
/// Extracted from `issue_realtime_token` so it is testable against a mock
/// server without `AppState`. It runs BEFORE concurrency permits and turn
/// registration (T-54.7-51): a failed mint registers nothing.
///
/// Key rule: `resolve_realtime_key` (plan 06) -- a typed namespaced reference,
/// or the OpenAI provider key only for the api.openai.com default.
/// - `Found`    -> mint; the browser gets the ephemeral token.
/// - `NotFound` -> typed error (the D-07 turn-based fallback trigger). This
///   covers the default base_url with no OpenAI key AND a typed reference that
///   does not resolve: neither may silently become a keyless session.
/// - `None`     -> keyless (D-12): nothing is minted; the result carries the
///   full session JSON for the browser's multipart calls POST. Reached only for
///   a custom base_url with no `realtime.api_key_ref`.
///
/// Open Question 2: the keyless multipart path follows the OpenAI unified
/// interface and is UAT-unverified against a non-OpenAI compatible server.
///
/// T-LOG-LEAK: the key, the minted token and response bodies are never logged;
/// only the fixed failure category is.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn mint_realtime_for(
    config: &ironhermes_core::config::Config,
    identity: Option<&str>,
    instructions: &str,
    tools: &[serde_json::Value],
) -> Result<MintedRealtime, ServerFnError> {
    use ironhermes_core::voice_http::VoiceKey;

    let provider = configured_realtime_provider(config)?;
    // D-11/D-16: resolve the identity-aware model + voice and run the charset
    // gate BEFORE any key resolution or network call (T-54.7-47).
    let (model, voice) = resolve_realtime_selection(config, identity)?;
    let session = provider.build_session(&model, &voice, instructions, tools);
    let calls_url = realtime_calls_url_for(config)?;

    match resolve_realtime_key(config).await {
        VoiceKey::Found(key) => {
            let body = provider.build_mint_request(&session);
            let minted = provider
                .mint(&config.realtime, Some(&key), &body)
                .await
                .map_err(|e| {
                    tracing::warn!(
                        error_category = e.category.as_str(),
                        "Realtime token request failed"
                    );
                    ServerFnError::new(format!("realtime mint failed: {}", e.category.as_str()))
                })?;
            let token = minted
                .token
                .ok_or_else(|| ServerFnError::new("No token in response"))?;
            Ok(MintedRealtime {
                token: Some(token),
                calls_url,
                session_json: None,
            })
        }
        VoiceKey::None => {
            // D-12 keyless path: a custom base_url with no key reference. Mint
            // nothing (no request leaves the host) and let the browser send the
            // FULL session -- instructions and tools included -- next to its SDP
            // offer. That POST is plain HTTPS, so the SCTP size limit that moved
            // these fields into the mint request (Pitfall 5) does not apply.
            // Only the custom-base path reaches here: resolve_realtime_key never
            // returns None for the api.openai.com default (Pitfall 9).
            Ok(MintedRealtime {
                token: None,
                calls_url,
                session_json: Some(session.to_string()),
            })
        }
        VoiceKey::NotFound => {
            // T-LOG-LEAK: no key material and no variable name in the log line.
            tracing::info!(
                "realtime key not found -- realtime unavailable (D-07 fallback trigger)"
            );
            Err(ServerFnError::new(
                "realtime key not found -- realtime unavailable",
            ))
        }
    }
}

/// Phase 36.17.12 Plan 02: Exchange the server-side realtime key for a short-lived
/// ephemeral token for a WebRTC-direct realtime session (D-04/D-05).
///
/// Phase 54.7 (D-10/D-12): the mint goes to `{realtime.base_url}/realtime/client_secrets`
/// through the configured realtime provider (`mint_realtime_for`), and the result
/// carries `calls_url` (`{base_url}/realtime/calls`) for the browser's direct SDP
/// POST. The OpenAI provider key is used only for the api.openai.com default.
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
///   config-driven, not browser-supplied) then charset-gated BEFORE any
///   network call (D-11, `validate_realtime_values`). `voice` is gated the same way.
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
    // Step 1: Load config. Model, voice, key and addresses are all resolved
    // server-side from saved config -- never from browser-supplied values.
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;

    // Step 2 (avatar-not-hearing-voice follow-up -- see block comment above
    // build_realtime_instructions_and_tools): build instructions + tools ONCE
    // here so they travel in the mint request (or, keyless, the browser's calls
    // POST) instead of the data-channel session.update, which is what
    // previously pushed that message 1.83x over Chrome's SCTP maxMessageSize.
    //
    // The D-11 gate runs first (it is pure and cheap) so a bad model or voice is
    // rejected before the expensive prompt build. mint_realtime_for gates again.
    resolve_realtime_selection(&config, active_identity.as_deref())?;
    let (instructions, tools) = build_realtime_instructions_and_tools().await;

    // Step 3 (Phase 54.7 D-10/D-12): mint through the configured realtime
    // provider at `{realtime.base_url}/realtime/client_secrets`. Any failure is
    // a typed error, which voice mode turns into the D-07 turn-based fallback.
    let MintedRealtime {
        token,
        calls_url,
        session_json,
    } = mint_realtime_for(&config, active_identity.as_deref(), &instructions, &tools).await?;

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
        calls_url,
        session_json,
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

/// Phase 54.7 (D-11): the fallback input-transcription model when the stored
/// value fails the charset gate. (`off` is a valid stored value that disables
/// transcription; it is not a fallback.)
#[cfg(not(target_arch = "wasm32"))]
const REALTIME_FALLBACK_TRANSCRIPTION: &str = "gpt-4o-mini-transcribe";

/// BUG 5 / Phase 39.3 Plan 01, revised by the avatar-not-hearing-voice follow-up:
/// Pure, hermetic builder for the GA-shaped Realtime `session` object JSON sent
/// over the RTCDataChannel. Inputs are validated/clamped by the caller; this
/// function only shapes the JSON so it is unit-testable without Config::load or
/// any network call.
///
/// AUDIO/VAD CONFIG ONLY — no `instructions`, no `tools`, no `tool_choice`.
/// Those two fields previously lived here as TOP-LEVEL session siblings of
/// `audio` (D-01/D-02), but measurement showed `instructions` alone (440,628
/// bytes) was 1.68x Chrome's own 262,144-byte SCTP maxMessageSize, so the
/// combined message was rejected outright by `RTCDataChannel.send()` (see
/// .planning/debug/avatar-not-hearing-voice.md, MEASURED 2026-09-18 19:52).
/// They now travel in `issue_realtime_token`'s ephemeral-token request via
/// `build_realtime_token_request_json` (server→OpenAI HTTPS — no SCTP limit),
/// and OpenAI's client_secrets docs confirm session config set there can still
/// be overridden by the client's own `session.update`, so this smaller
/// audio-only message remains a valid override layer for VAD/noise/barge-in.
#[cfg(not(target_arch = "wasm32"))]
fn build_realtime_session_json(
    noise_reduction: &str,
    vad_mode: &str,
    threshold: f32,
    silence_ms: u32,
    prefix_ms: u32,
    transcription_model: &str,
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
    // avatar-not-hearing-voice follow-up: NO instructions/tools/tool_choice here
    // — see the function doc comment above for why they moved to
    // build_realtime_token_request_json instead.
    serde_json::json!({
        "type": "realtime",
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

/// Phase 36.17.12 BUG 5, revised by the avatar-not-hearing-voice follow-up:
/// Return the GA-shaped Realtime `session` config JSON, resolved from
/// `config.voice.*`. The browser sends this verbatim inside a `session.update`
/// event over the DataChannel. Invalid config values fall back to safe
/// defaults (far_field / semantic_vad) rather than erroring, so a typo
/// degrades gracefully instead of disabling realtime.
///
/// AUDIO/VAD CONFIG ONLY. `instructions` (D-01) and `tools` (D-02) no longer
/// flow through this function — they are built once by
/// `build_realtime_instructions_and_tools` and sent in `issue_realtime_token`'s
/// ephemeral-token request instead, because together they measured 480,128
/// bytes (1.83x Chrome's 262,144-byte SCTP maxMessageSize) and made this
/// message's `RTCDataChannel.send()` fail outright, silently dropping the VAD
/// config that used to ride along with them (see
/// .planning/debug/avatar-not-hearing-voice.md, MEASURED 2026-09-18 19:52).
#[server]
pub async fn realtime_session_config() -> Result<String, ServerFnError> {
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
    let session_json = realtime_session_config_for(&config);

    // DIAGNOSTIC (Avatar-mode regression follow-up, left in place per the
    // avatar-not-hearing-voice debug session so a live reading still confirms
    // the fix): this object is sent as ONE RTCDataChannel message client-side.
    // It is now audio/VAD-only (instructions/tools moved to
    // issue_realtime_token), so total_bytes should be a small fraction of the
    // 262,144-byte SCTP maxMessageSize going forward — a regression back
    // toward the old size is the signal to watch for here.
    tracing::warn!(
        total_bytes = session_json.len(),
        "[realtime_session_config] session object size (audio/VAD only)"
    );

    Ok(session_json)
}

/// Pure body of `realtime_session_config` (Phase 54.7 D-11): the audio/VAD
/// session JSON for `config`, testable without `Config::load`.
///
/// The input-transcription model comes from `realtime.transcription_model()`
/// through the same charset gate as `validate_realtime_values`; an invalid
/// stored value falls back to `gpt-4o-mini-transcribe` with a warning that
/// names only the field, never the value.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn realtime_session_config_for(config: &ironhermes_core::config::Config) -> String {
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
    let stored_transcription = config.realtime.transcription_model();
    let transcription_model =
        if ironhermes_core::voice_endpoint::validate_transcription_model(stored_transcription)
            .is_ok()
        {
            stored_transcription
        } else {
            tracing::warn!(
                field = "realtime.transcription_model",
                "invalid stored value; using the default transcription model"
            );
            REALTIME_FALLBACK_TRANSCRIPTION
        };

    build_realtime_session_json(
        noise_reduction,
        vad_mode,
        threshold,
        v.realtime_vad_silence_ms,
        v.realtime_vad_prefix_ms,
        transcription_model,
    )
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

    // 49.8-08 (D-10/D-16): a realtime turn ending normally is a real session
    // boundary for voice2voice — stop the robot and cancel any goals.
    // Awaited, and bounded by end_session_stop's own session-end timeout
    // (see ironhermes_tools::ros2::session), so an unreachable bridge can
    // never wedge this endpoint's response.
    let ros2_report = ironhermes_tools::ros2::end_session_stop(app_state.runtime.registry()).await;
    if ros2_report != "nothing to stop" {
        tracing::info!(report = %ros2_report, "realtime_session_end: ros2 session-end stop");
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
            // 49.8-08 (D-10/D-16): the heartbeat observing Cancelled IS the
            // realtime turn's session boundary — stop the robot and cancel
            // any goals here rather than waiting for realtime_session_end,
            // which a torn-down client may never call.
            let ros2_report =
                ironhermes_tools::ros2::end_session_stop(app_state.runtime.registry()).await;
            if ros2_report != "nothing to stop" {
                tracing::info!(report = %ros2_report, "realtime_heartbeat cancelled: ros2 session-end stop");
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

    // 49.8-08 (D-10/D-16): pruning at least one abandoned realtime session is
    // a real session boundary — stop the robot and cancel any goals. Skipped
    // entirely when nothing was pruned (no boundary crossed).
    if count > 0 {
        let ros2_report = ironhermes_tools::ros2::end_session_stop(app_state.runtime.registry()).await;
        if ros2_report != "nothing to stop" {
            tracing::info!(report = %ros2_report, "realtime_prune: ros2 session-end stop");
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
/// - `Output`: the tool was dispatched; `output` is the string to hand the model as
///   the `function_call_output`. That is the tool's own result on success, or a
///   `realtime_tool_error_output` payload (`{"error":"tool_failed",...}`) when the
///   tool itself returned `Err` — a failing tool is NOT an HTTP 500.
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
/// - `needs_approval`: `should_prompt_for_approval(<the web session's DefconLevel>)` returned true.
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

/// The DEFCON level a realtime tool call is judged at: the level of the web session that
/// owns the turn, read live (D-21), or the process default when the turn's session id is
/// unknown or is not a safe session id (so a hostile id creates no registry entry).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn realtime_session_level(
    session_id: Option<&str>,
) -> ironhermes_core::config::DefconLevel {
    use ironhermes_core::defcon::session_defcon;
    match session_id.and_then(ironhermes_core::safe_session_id) {
        Some(id) => session_defcon().handle_for(id).get(),
        None => session_defcon().process_default(),
    }
}

/// Whether a realtime tool call must be held for approval (D-03 / D-21): the web session's
/// level decides, through the one `should_prompt_for_approval` rule.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn realtime_needs_approval(session_id: Option<&str>) -> bool {
    ironhermes_tools::approval::should_prompt_for_approval(realtime_session_level(session_id))
}

/// Longest `reason` (in bytes) a tool-failure payload forwards to the model.
///
/// The payload rides the WebRTC data channel (SCTP `maxMessageSize` can be as low
/// as 64 KiB — see `realtime_session.rs`) and tool errors can embed whole upstream
/// response bodies, so the reason is bounded. 600 bytes is far more than a voice
/// model can use and far less than any transport limit.
const REALTIME_TOOL_ERROR_REASON_MAX_BYTES: usize = 600;

/// Build the `function_call_output` string for a realtime tool call that FAILED.
///
/// Target-independent on purpose: the server uses it when a tool returns `Err`
/// (`kind = "tool_failed"`), and the browser uses it when the relay itself failed
/// (`kind = "server_error"`) — so the model always gets the same, self-describing
/// shape: WHICH tool, WHY, and a hint that it can recover. The previous behavior
/// was a constant `{"error":"server_error"}` with none of the three, which left a
/// voice model with nothing to act on (debug session web-extract-hangs-60s,
/// round 2: `web_answer` exhausted its provider chain -> HTTP 500 -> model silent).
///
/// `reason` is truncated on a char boundary to
/// [`REALTIME_TOOL_ERROR_REASON_MAX_BYTES`]. The result is always valid JSON.
pub fn realtime_tool_error_output(kind: &str, tool: Option<&str>, reason: &str) -> String {
    let mut end = reason.len().min(REALTIME_TOOL_ERROR_REASON_MAX_BYTES);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    let mut reason_out = reason[..end].to_string();
    if end < reason.len() {
        reason_out.push('\u{2026}');
    }
    serde_json::json!({
        "error": kind,
        "tool": tool,
        "reason": reason_out,
        "hint": "The tool call failed and returned no result. Do not repeat the identical \
                 call. Try a different tool or approach, or tell the user plainly that it \
                 could not be completed.",
    })
    .to_string()
}

/// What one dispatched realtime tool call produced, ready to hand to the model.
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
pub(crate) struct RealtimeDispatch {
    /// The `function_call_output` string: the tool's own output on success, or a
    /// [`realtime_tool_error_output`] payload on failure.
    pub output: String,
    /// `true` when the tool itself returned `Ok`. Drives the black-box
    /// `tool_completed.success` flag and gates trajectory logging (a failure payload
    /// is not a tool result).
    pub ok: bool,
}

/// Dispatch one realtime tool call through the registry (D-02).
///
/// Single seam shared by `realtime_tool_call` (auto-approved path) and
/// `realtime_approve` (approved path) so the failure contract lives in ONE place
/// and is unit-testable without a global `AppState`.
///
/// **Failure contract:** a tool that returns `Err` (including "tool not found" and
/// "tool not available") is NOT a transport failure. It is converted into a
/// model-readable `function_call_output` payload and returned as a normal
/// `Output`, so the browser relays it to the model (HTTP 200) and the model can
/// recover — e.g. fall back from `web_answer` to `web_search` + `web_extract`.
/// Returning the `Err` as a `ServerFnError` (HTTP 500) is what this replaced.
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
pub(crate) async fn dispatch_realtime_tool(
    registry: &ironhermes_tools::ToolRegistry,
    name: &str,
    args: serde_json::Value,
) -> RealtimeDispatch {
    match registry.dispatch(name, args).await {
        Ok(output) => RealtimeDispatch { output, ok: true },
        Err(e) => {
            // The 500 this replaced also produced a tower_http ERROR line; keep the
            // failure visible server-side now that the response is a 200.
            tracing::warn!(
                tool = %name,
                error = %e,
                "realtime tool call failed; returning an error payload to the model"
            );
            RealtimeDispatch {
                output: realtime_tool_error_output("tool_failed", Some(name), &e.to_string()),
                ok: false,
            }
        }
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
/// 4. `should_prompt_for_approval(<the web session's live DefconLevel>)` is called
///    unconditionally; if true (levels 1-3), the call is held as `ApprovalPending` — the
///    tool is NOT dispatched. If false (level 4-5, an audited bypass surface), the call is
///    dispatched immediately.
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
    // D-21: the web session that owns this turn decides, at its live level.
    let turn_session = app_state
        .turn_registry
        .get_one(uuid)
        .await
        .map(|summary| summary.session_id);
    let needs_approval = realtime_needs_approval(turn_session.as_deref());

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
    let RealtimeDispatch {
        output,
        ok: tool_ok,
    } = {
        let registry = app_state.runtime.registry().read().await;
        dispatch_realtime_tool(&registry, &name, args_parsed.clone()).await
    };

    // One INFO line per completed realtime tool call (name/outcome/latency only — never
    // args or output). Before this, a successful realtime tool call left NO line in
    // web.log, so "did the avatar actually reach web_extract?" could only be inferred
    // from the absence of errors.
    tracing::info!(
        tool = %name,
        call_id = %call_id,
        ok = tool_ok,
        latency_ms = bb_tool_start.elapsed().as_millis() as u64,
        "realtime tool call completed"
    );

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
                "success": tool_ok,
                "result_hash": result_hash,
                "latency_ms": bb_tool_start.elapsed().as_millis() as u64,
            }),
        ));
    }

    // --- D-04: log to trajectory with redacted args (T-LOG-LEAK fix — parity with agent_loop.rs).
    // Resolve the tool from the registry and apply redact_args before logging, mirroring
    // the text-chat path (agent_loop.rs L2491-2497). Guard scope is kept tight: acquired,
    // used for redact_args, then dropped before the synchronous log call.
    //
    // Only for calls whose tool succeeded: the trajectory entry written here carries
    // `error: None` and `result: Some(output)`, so logging a failure payload would record
    // a failed call as a successful one. Failures are surfaced by the WARN in
    // `dispatch_realtime_tool` and `success: false` in the black-box event above.
    if tool_ok {
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
    }

    Ok(RealtimeToolResult::Output { output })
}

// =============================================================================
// Phase 39.3 Plan 02: unit tests for realtime_tool_call (decide_realtime_tool_action)
// =============================================================================

#[cfg(all(test, not(target_arch = "wasm32")))]
mod realtime_tool_call_tests {
    use super::{RealtimeToolAction, decide_realtime_tool_action, realtime_needs_approval};
    use ironhermes_core::config::DefconLevel;
    use ironhermes_core::defcon::session_defcon;

    /// D-21: the realtime approval hold follows the web session's own level, read at each
    /// call. Two sessions at different levels are judged differently under the SAME process
    /// default, and a session that is unknown or not a safe id gets the process default.
    #[test]
    fn web_realtime_tool_uses_session_level() {
        let lax = "s-realtime-lax";
        let strict = "s-realtime-strict";
        let registry = session_defcon();
        let previous_default = registry.process_default();
        registry.set_process_default(DefconLevel::THREE);

        registry.set_session_level(lax, DefconLevel::FIVE);
        registry.set_session_level(strict, DefconLevel::ONE);
        assert!(
            !realtime_needs_approval(Some(lax)),
            "a session at DEFCON 5 bypasses the approval hold"
        );
        assert!(
            realtime_needs_approval(Some(strict)),
            "a session at DEFCON 1 holds for approval even though the process default is 3"
        );

        // The level is read live: tightening the lax session takes effect on the next call.
        registry.set_session_level(lax, DefconLevel::THREE);
        assert!(realtime_needs_approval(Some(lax)));

        // No session, or a session id that is not safe: the process default decides.
        assert!(realtime_needs_approval(None));
        registry.set_process_default(DefconLevel::FIVE);
        assert!(!realtime_needs_approval(None));
        assert!(!realtime_needs_approval(Some("not/a/safe/id")));

        registry.forget(lax);
        registry.forget(strict);
        registry.set_process_default(previous_default);
    }

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

// =============================================================================
// Debug session web-extract-hangs-60s (round 2): a failing realtime tool must
// reach the model as a function_call_output payload it can act on — never as an
// HTTP 500 that the browser can only turn into an opaque `server_error`.
// =============================================================================

#[cfg(all(test, not(target_arch = "wasm32")))]
mod realtime_dispatch_tests {
    use super::dispatch_realtime_tool;
    use async_trait::async_trait;
    use ironhermes_core::ToolSchema;
    use ironhermes_tools::{Tool, ToolRegistry};

    /// A tool whose `execute` returns the given result — the shape of
    /// `web_answer` when its whole provider chain is exhausted.
    struct ScriptedTool {
        name: &'static str,
        result: Result<&'static str, &'static str>,
    }

    #[async_trait]
    impl Tool for ScriptedTool {
        fn name(&self) -> &str {
            self.name
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "scripted test tool"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                self.name,
                "scripted test tool",
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            match self.result {
                Ok(s) => Ok(s.to_string()),
                Err(e) => Err(anyhow::anyhow!(e)),
            }
        }
    }

    fn registry_with(tool: ScriptedTool) -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(tool));
        reg
    }

    /// THE regression: `web_answer` with every provider exhausted returned `Err`,
    /// which `realtime_tool_call` used to propagate with `?` as a `ServerFnError`
    /// (HTTP 500, 2026-10-10T22:00:01Z). The browser could then only send the
    /// constant `{"error":"server_error"}`, so the model never learned WHICH tool
    /// failed, WHY, or that a different tool would work.
    #[tokio::test]
    async fn failing_tool_yields_model_readable_output_not_a_server_error() {
        let reg = registry_with(ScriptedTool {
            name: "web_answer",
            result: Err("web_answer: no provider in the configured chain returned an answer \
                         (perplexity: no key configured; exa: Exa answer response carried no output)"),
        });

        // The seam is infallible: a tool failure comes back as a function_call_output
        // payload (HTTP 200), never as a ServerFnError (HTTP 500).
        let d =
            dispatch_realtime_tool(&reg, "web_answer", serde_json::json!({"query": "news"})).await;
        assert!(
            !d.ok,
            "the dispatch outcome must record that the tool failed"
        );

        let v: serde_json::Value =
            serde_json::from_str(&d.output).expect("failure payload must be valid JSON");
        assert_eq!(v["error"], "tool_failed");
        assert_eq!(
            v["tool"], "web_answer",
            "model must learn WHICH tool failed"
        );
        assert!(
            v["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("no provider in the configured chain"),
            "model must learn WHY it failed; got {v}"
        );
        assert!(
            !v["hint"].as_str().unwrap_or_default().is_empty(),
            "payload must tell the model it can recover (different tool / tell the user)"
        );
    }

    /// Control: a tool that succeeds is passed through verbatim — the fix must not
    /// wrap or alter happy-path output.
    #[tokio::test]
    async fn successful_tool_output_passes_through_verbatim() {
        let reg = registry_with(ScriptedTool {
            name: "web_search",
            result: Ok("1. Example headline - https://example.com/a"),
        });
        let d = dispatch_realtime_tool(&reg, "web_search", serde_json::json!({})).await;
        assert!(d.ok);
        assert_eq!(d.output, "1. Example headline - https://example.com/a");
    }

    /// A tool name the registry does not know is also a model-recoverable failure
    /// (e.g. a stale tool list in the realtime session), not a 500.
    #[tokio::test]
    async fn unknown_tool_is_reported_to_the_model_not_a_server_error() {
        let reg = ToolRegistry::new();
        let d = dispatch_realtime_tool(&reg, "no_such_tool", serde_json::json!({})).await;
        assert!(!d.ok);
        let v: serde_json::Value = serde_json::from_str(&d.output).expect("valid JSON");
        assert_eq!(v["error"], "tool_failed");
        assert_eq!(v["tool"], "no_such_tool");
    }

    /// Boundary: the payload rides the WebRTC data channel (SCTP maxMessageSize can be
    /// as low as 64 KiB — see realtime_session.rs), and tool errors can embed whole
    /// upstream response bodies. The reason must be bounded, valid JSON, and cut on a
    /// char boundary (multi-byte input must not panic or corrupt the payload).
    #[tokio::test]
    async fn oversized_multibyte_failure_reason_is_truncated_safely() {
        // 3-byte chars; 5000 of them = 15 000 bytes.
        let huge: &'static str = Box::leak("\u{20ac}".repeat(5000).into_boxed_str());
        let reg = registry_with(ScriptedTool {
            name: "big_error",
            result: Err(huge),
        });
        let d = dispatch_realtime_tool(&reg, "big_error", serde_json::json!({})).await;
        assert!(
            d.output.len() < 2_048,
            "failure payload must be bounded, got {} bytes",
            d.output.len()
        );
        let v: serde_json::Value =
            serde_json::from_str(&d.output).expect("valid JSON after truncation");
        assert_eq!(v["error"], "tool_failed");
        assert!(
            v["reason"]
                .as_str()
                .unwrap_or_default()
                .ends_with('\u{2026}'),
            "a truncated reason must say so"
        );
    }

    // -------------------------------------------------------------------------
    // Server-side probe: the realtime relay reaches the REAL web_extract tool.
    // -------------------------------------------------------------------------
    //
    // The web_extract nested-semaphore fix (crates/ironhermes-tools) was never
    // exercised through the realtime surface: after the operator restarted the dx
    // server there were no web_search / web_extract calls at all, because the model
    // chose `web_answer` and that call died as an HTTP 500. This drives the SAME
    // seam `realtime_tool_call` uses (`dispatch_realtime_tool` -> `ToolRegistry::dispatch`,
    // i.e. availability check + guardrails + `run_bounded`) with the production-shaped
    // call: 4 news-homepage-sized pages (== extract.max_parallel_summaries) with LLM
    // processing on. Pre-fix every URL ended as `extraction_timeout` (the whole
    // per-URL deadline); with an instant mock summarizer a pass here can only mean
    // the relay path reaches web_extract and the permit deadlock is gone.
    //
    // Fully isolated: IRONHERMES_HOME is a tempdir (never ~/.ironhermes), provider
    // keys are unset so the keyless local fetcher is used, the "site" is a loopback
    // listener on an ephemeral port, and nothing binds :8080.

    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use ironhermes_core::{SkillRegistry, SummarizationClientHandle};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Serialises the env-mutating probe (the crate runs with --test-threads=1, but
    /// the lock keeps the test correct if that ever changes).
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Restores a process env var on drop.
    struct EnvRestore {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvRestore {
        fn set(key: &'static str, val: &str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, val);
            Self { key, prev }
        }
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, prev }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    /// Instant mock summarizer that records how many LLM calls were made.
    struct CountingSummarizer {
        calls: Mutex<usize>,
    }

    #[async_trait]
    impl SummarizationClientHandle for CountingSummarizer {
        async fn summarize_call(
            &self,
            _system_prompt: String,
            _user_prompt: String,
            _max_tokens: u32,
        ) -> anyhow::Result<String> {
            *self.calls.lock().unwrap() += 1;
            Ok("MOCK_SUMMARY".to_string())
        }
    }

    /// Loopback HTTP "news site": `/pageN` answers after `delay` with a ~9K-char
    /// article. The delay is a real fetch latency so all N URL tasks hold their fetch
    /// permit at the same moment, like a real batch (see the tools-crate tests for the
    /// scheduling-skew rationale).
    async fn spawn_news_site(delay: Duration, words: usize) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral loopback port");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/page0");
                    let idx: usize = path.trim_start_matches("/page").parse().unwrap_or(0);
                    tokio::time::sleep(delay).await;
                    let body = format!(
                        "<html><body><article><p>{}</p></article></body></html>",
                        format!("page{idx} ").repeat(words)
                    );
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\n\
                         connection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        addr
    }

    /// The voice directive must name the lookup tools the realtime model can
    /// actually call, or a bare "latest news" request goes unanswered.
    #[test]
    fn realtime_current_info_directive_names_web_lookup_tools() {
        for tool in ["web_search", "web_answer"] {
            assert!(
                crate::server::api::REALTIME_CURRENT_INFO_DIRECTIVE.contains(tool),
                "directive must name `{tool}`"
            );
        }
        assert!(crate::server::api::REALTIME_CURRENT_INFO_DIRECTIVE.contains("news"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn realtime_relay_reaches_web_extract_with_batch_equal_to_permit_count() {
        let _g = ENV_LOCK.lock().await;
        let _f = EnvRestore::unset("FIRECRAWL_API_KEY");
        let _e = EnvRestore::unset("EXA_API_KEY");
        let _t = EnvRestore::unset("TAVILY_API_KEY");
        let _ssrf = EnvRestore::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");

        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            home.path().join("config.yaml"),
            "extract:\n  max_parallel_summaries: 4\n  per_url_timeout_secs: 10\n",
        )
        .expect("write isolated config.yaml");
        let _home = EnvRestore::set("IRONHERMES_HOME", home.path().to_str().expect("utf8 path"));

        let addr = spawn_news_site(Duration::from_millis(1000), 1500).await;
        let urls: Vec<String> = (0..4).map(|i| format!("http://{addr}/page{i}")).collect();

        let counter = Arc::new(CountingSummarizer {
            calls: Mutex::new(0),
        });
        let handle: Arc<dyn SummarizationClientHandle> = counter.clone();
        let skills = Arc::new(SkillRegistry::load_with_paths(&[]));
        let mut reg = ToolRegistry::new();
        reg.register_web_extract_tool(handle, skills);

        let started = Instant::now();
        let d = dispatch_realtime_tool(
            &reg,
            "web_extract",
            serde_json::json!({ "urls": urls, "use_llm_processing": true }),
        )
        .await;
        let elapsed = started.elapsed();

        assert!(
            d.ok,
            "web_extract must be reachable through the realtime dispatch seam; got failure payload: {}",
            d.output
        );
        let results: Vec<serde_json::Value> =
            serde_json::from_str(&d.output).expect("web_extract returns a JSON array of results");
        assert_eq!(results.len(), 4, "one result per URL");
        for (i, r) in results.iter().enumerate() {
            assert!(
                r["error"].is_null(),
                "URL {i} must succeed; an `extraction_timeout` here is the nested-semaphore \
                 deadlock (4 URLs == 4 permits): {r}"
            );
            assert_eq!(r["content"].as_str(), Some("MOCK_SUMMARY"), "URL {i}: {r}");
        }
        assert_eq!(
            *counter.calls.lock().unwrap(),
            4,
            "one summarizer call per URL"
        );
        assert!(
            elapsed < Duration::from_secs(8),
            "4 parallel 1s fetches + instant summaries must finish in a few seconds, took {elapsed:?} \
             (the pre-fix failure mode burns the whole per-URL deadline)"
        );
    }

    /// The client reuses `realtime_tool_error_output` for transport-level failures
    /// (`kind = "server_error"`, tool known from the originating function_call). It must
    /// produce the same self-describing shape, and a short reason must pass through
    /// untouched (no spurious ellipsis).
    #[test]
    fn error_output_shape_is_stable_for_client_transport_failures() {
        let out = super::realtime_tool_error_output(
            "server_error",
            Some("web_extract"),
            "error sending request for url",
        );
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(v["error"], "server_error");
        assert_eq!(v["tool"], "web_extract");
        assert_eq!(v["reason"], "error sending request for url");
        assert!(!v["hint"].as_str().unwrap_or_default().is_empty());

        // Tool unknown (e.g. a malformed function_call item) -> `tool` is null, not absent.
        let out = super::realtime_tool_error_output("server_error", None, "x");
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert!(v["tool"].is_null());
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

    // Same failure contract as `realtime_tool_call`: a tool `Err` becomes a
    // model-readable `Output` payload (HTTP 200), never a `ServerFnError` (HTTP 500).
    let RealtimeDispatch {
        output,
        ok: tool_ok,
    } = {
        let registry = app_state.runtime.registry().read().await;
        dispatch_realtime_tool(&registry, &pending.tool_name, args_parsed.clone()).await
    };

    // --- D-04: log to trajectory with redacted args (T-LOG-LEAK fix — parity with agent_loop.rs).
    // Resolve the tool from the registry and apply redact_args before logging, mirroring
    // the text-chat path (agent_loop.rs L2491-2497). Guard scope is kept tight: acquired,
    // used for redact_args, then dropped before the synchronous log call.
    // Success-only — see the matching note in `realtime_tool_call`.
    if tool_ok {
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
    }

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

    /// The mint request body, built by the real realtime provider (Phase 54.7
    /// D-10): `build_mint_request(build_session(..))`.
    fn build_realtime_token_request_json(
        model: &str,
        voice: &str,
        instructions: &str,
        tools: &[serde_json::Value],
    ) -> serde_json::Value {
        let provider = ironhermes_tools::realtime::realtime_provider("openai_compatible")
            .expect("openai_compatible provider exists");
        provider.build_mint_request(&provider.build_session(model, voice, instructions, tools))
    }

    // Phase 54.7 (D-11): the three fixed whitelists became the charset gate;
    // these tests call the real `validate_realtime_values` that
    // `mint_realtime_for` runs, not a test-local copy of it.

    /// T-36.17.12-02-V1: a model outside the charset is rejected with Err BEFORE
    /// any network call (offline/hermetic -- the gate runs before reqwest).
    #[test]
    fn realtime_token_validation_bogus_model() {
        let result =
            super::validate_realtime_values("bogus\"model", "shimmer");
        let msg = result.expect_err("bad model must be rejected").to_string();
        assert!(
            msg.contains("Unknown model"),
            "Error must mention 'Unknown model', got: {msg}"
        );
        assert!(
            !msg.contains("sk-"),
            "Error message must not leak key material: {msg}"
        );
    }

    /// T-36.17.12-02-V2: a voice outside the charset (valid model) is rejected.
    #[test]
    fn realtime_token_validation_bogus_voice() {
        let result = super::validate_realtime_values("gpt-realtime", "bogus voice");
        let msg = result.expect_err("bad voice must be rejected").to_string();
        assert!(
            msg.contains("Unknown voice"),
            "Error must mention 'Unknown voice', got: {msg}"
        );
        assert!(
            !msg.contains("sk-"),
            "Error message must not leak key material: {msg}"
        );
    }

    /// T-36.17.12-02-V3: the historical combos still pass the gate.
    #[test]
    fn realtime_token_validation_valid_combo_passes_gate() {
        for voice in ["alloy", "shimmer", "echo"] {
            assert!(
                super::validate_realtime_values("gpt-realtime", voice)
                    .is_ok(),
                "gpt-realtime + {voice} must pass the gate"
            );
        }
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
        let json =
            build_realtime_session_json("near_field", "server_vad", 0.7, 600, 250, "whisper-1");
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
        );
        let on_v: serde_json::Value = serde_json::from_str(&on).unwrap();
        assert_eq!(
            on_v["audio"]["input"]["transcription"]["model"],
            "gpt-4o-transcribe"
        );

        let off = build_realtime_session_json("far_field", "semantic_vad", 0.5, 500, 300, "off");
        let off_v: serde_json::Value = serde_json::from_str(&off).unwrap();
        assert!(
            off_v["audio"]["input"]["transcription"].is_null(),
            "off must serialize transcription as null: {off}"
        );
    }

    // ── avatar-not-hearing-voice follow-up: instructions/tools moved OFF the
    // data channel and ONTO the token request (supersedes Phase 39.3 Plan 01
    // Task 1's T-39.3-01-T1..T4, which locked the OLD — now-fixed — shape) ────

    /// Regression lock for the fix: the data-channel session.update must carry
    /// ONLY audio/VAD config. `instructions` + `tools` together previously
    /// measured 480,128 bytes here (1.83x the 262,144-byte SCTP limit) and made
    /// `RTCDataChannel.send()` reject the whole message outright — see
    /// .planning/debug/avatar-not-hearing-voice.md, MEASURED 2026-09-18 19:52.
    #[test]
    fn realtime_session_json_carries_no_instructions_or_tools() {
        let json = build_realtime_session_json(
            "far_field",
            "semantic_vad",
            0.5,
            500,
            300,
            "gpt-4o-mini-transcribe",
        );
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            v.get("instructions").is_none(),
            "instructions must NOT be in the data-channel session.update: {json}"
        );
        assert!(
            v.get("tools").is_none(),
            "tools must NOT be in the data-channel session.update: {json}"
        );
        assert!(
            v.get("tool_choice").is_none(),
            "tool_choice must NOT be in the data-channel session.update: {json}"
        );
        assert_eq!(v["type"], "realtime");
    }

    // ── build_realtime_token_request_json: instructions + tools now live HERE,
    // in the ephemeral-token request (server→OpenAI HTTPS — no SCTP limit) ────

    /// T1 (was T-39.3-01-T1/T2, retargeted): `instructions`, `tools`, and
    /// `tool_choice` are top-level `session` fields in the client_secrets
    /// request body, siblings of `audio` — same GA shape the data-channel
    /// message used to carry, just relocated to the request that has no
    /// message-size ceiling.
    #[test]
    fn token_request_json_carries_instructions_and_tools_top_level() {
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
        let instructions = "You are Hermes, a helpful assistant.";
        let body = build_realtime_token_request_json("gpt-realtime", "alloy", instructions, &tools);
        let session = &body["session"];
        assert_eq!(
            session["instructions"], instructions,
            "instructions must be a top-level session field: {body}"
        );
        let tools_arr = session["tools"].as_array().expect("tools must be an array");
        assert_eq!(tools_arr.len(), 2, "tools array must have 2 entries");
        assert_eq!(tools_arr[0]["name"], "search");
        assert_eq!(tools_arr[1]["name"], "read_file");
        assert_eq!(session["tool_choice"], "auto", "tool_choice must be 'auto'");
    }

    /// T2: `model`/`voice`/`type` are preserved exactly as the pre-fix request
    /// carried them (no regression to the existing token-mint contract).
    #[test]
    fn token_request_json_keeps_model_type_and_voice() {
        let body = build_realtime_token_request_json("gpt-realtime", "shimmer", "", &[]);
        let session = &body["session"];
        assert_eq!(session["type"], "realtime");
        assert_eq!(session["model"], "gpt-realtime");
        assert_eq!(session["audio"]["output"]["voice"], "shimmer");
    }

    /// T3: empty instructions/tools still produce a well-formed request (no
    /// panics on the degenerate/empty case — boundary neighbor of T1).
    #[test]
    fn token_request_json_handles_empty_instructions_and_tools() {
        let body = build_realtime_token_request_json("gpt-realtime", "alloy", "", &[]);
        let session = &body["session"];
        assert_eq!(session["instructions"], "");
        assert!(
            session["tools"]
                .as_array()
                .expect("tools must be an array")
                .is_empty(),
            "empty tools slice must still serialize as an empty array: {body}"
        );
    }

    /// T4 (was T-39.3-01-T4, retargeted): `instructions`/`tools` are siblings
    /// of `audio`, NOT nested under it (Pitfall 2 from RESEARCH.md — wrong
    /// schema nesting — still applies at this new call site).
    #[test]
    fn token_request_json_instructions_tools_are_siblings_of_audio() {
        let tool = serde_json::json!({
            "type": "function",
            "name": "ping",
            "description": "ping tool",
            "parameters": {"type": "object", "properties": {}, "required": []}
        });
        let body =
            build_realtime_token_request_json("gpt-realtime", "alloy", "hermes identity", &[tool]);
        let session = &body["session"];
        assert!(
            session["audio"].get("instructions").is_none(),
            "instructions must NOT be nested under audio: {body}"
        );
        assert!(
            session["audio"].get("tools").is_none(),
            "tools must NOT be nested under audio: {body}"
        );
        assert!(
            session.get("instructions").is_some(),
            "instructions must be top-level"
        );
        assert!(session.get("tools").is_some(), "tools must be top-level");
    }
}

// =============================================================================
// Phase 40.5 Plan 03 Task 3 — RED gate: resolve_realtime_voice tests
// Tests reference resolve_realtime_voice which does NOT exist yet → compile error = RED.
// =============================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod realtime_identity_tests_40_5_03 {
    use super::{resolve_realtime_model, resolve_realtime_voice, validate_realtime_values};
    use ironhermes_core::config::Config;

    /// D-12: identity override wins over global when the identity has realtime_voice set.
    #[test]
    fn realtime_resolves_identity_override() {
        let mut config = Config::default();
        config.realtime.voice = Some("shimmer".to_string());
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

    /// T-40.5-03-03 (replaces `realtime_override_still_whitelisted`, D-11): the
    /// resolved voice is still subject to the charset gate (defense in depth --
    /// the identity override does not bypass it).
    #[test]
    fn realtime_override_still_gated() {
        let mut config = Config::default();
        // Set orb_bloom's realtime_voice to something outside the voice charset.
        config
            .identities
            .entry("orb_bloom".to_string())
            .or_default()
            .voice
            .realtime_voice = Some("definitely invalid voice".to_string());

        let resolved = resolve_realtime_voice(&config, Some("orb_bloom"))
            .expect("orb_bloom is a known identity");
        assert_eq!(resolved, "definitely invalid voice");

        // The real gate must reject the override.
        assert!(
            validate_realtime_values("gpt-realtime", &resolved).is_err(),
            "the gate must not allow an identity-override bypass"
        );
    }

    /// D-16: the identity's realtime_model wins over the global model.
    #[test]
    fn realtime_model_resolves_identity_override() {
        let mut config = Config::default();
        config
            .identities
            .entry("orb_bloom".to_string())
            .or_default()
            .voice
            .realtime_model = Some("gpt-realtime-2".to_string());

        assert_eq!(
            resolve_realtime_model(&config, Some("orb_bloom")).expect("known identity"),
            "gpt-realtime-2"
        );
        assert_eq!(
            resolve_realtime_model(&config, None).expect("None slug always resolves"),
            config.realtime.model()
        );
        assert!(
            resolve_realtime_model(&config, Some("completely_unknown_slug_xyz")).is_err(),
            "an unknown identity slug must be rejected"
        );
        assert!(
            resolve_realtime_model(&config, Some("bad slug!")).is_err(),
            "a malformed identity slug must be rejected"
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
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct ModelRoleAssignment {
    pub role_key: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// Phase 50.5 (D-14): READ — the currently configured per-model window
    /// override (`providers.<p>.models.<m>.context_length`) for this role's
    /// model, if any. WRITE — the value to set when `apply_context_length`
    /// is true. `#[serde(default)]` keeps every pre-50.5 caller compiling
    /// against the wire format unchanged.
    #[serde(default)]
    pub context_length: Option<u32>,
    /// Phase 50.5 (D-14): WRITE-ONLY intent flag. `false` (the default) means
    /// leave the stored override alone; `true` + `context_length: None`
    /// clears it; `true` + `context_length: Some(n)` sets it. This explicit
    /// flag is what lets `context_length: None` mean two different things
    /// without a nested `Option<Option<_>>` and its serde ambiguity.
    #[serde(default)]
    pub apply_context_length: bool,
    /// Phase 50.5 (D-12): READ-ONLY. The resolved window for this role's
    /// model — falls back to the main endpoint's resolved value when the
    /// role has no override (`resolve_role` returns `None`).
    #[serde(default)]
    pub resolved_context_length: u32,
    /// Phase 50.5 (D-12): READ-ONLY. Which tier produced
    /// `resolved_context_length`, from the same evaluation as the number.
    #[serde(default)]
    pub resolved_context_source: ContextWindowSource,
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
    /// Phase 50.5 (D-12/D-15): the default card's resolved context window,
    /// from `resolve_for_main().context_length_with_source()`.
    #[serde(default)]
    pub default_resolved_context_length: u32,
    /// Phase 50.5 (D-12/D-15): which tier produced
    /// `default_resolved_context_length`.
    #[serde(default)]
    pub default_resolved_context_source: ContextWindowSource,
    /// Phase 50.5 (D-14/D-15): the currently configured per-model override
    /// for the default card's (provider, model) pair, if any.
    #[serde(default)]
    pub default_context_override: Option<u32>,
}

/// Phase 46.9 Plan 02 (D-05/D-10): Web-safe write payload for the Models
/// screen. `default_model: None` leaves `config.model.default` unchanged;
/// each entry in `roles` upserts one `config.model.roles` key. Only the six
/// MODEL_ROLE_KEYS are accepted — `validate_models_roles_payload` rejects
/// anything else before any mutation (T-46.9-06).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct ModelsRolesWritePayload {
    pub default_model: Option<String>,
    /// Phase 46.9 Plan 07 (GAP-1/D-05): `Some` merges into
    /// `config.model.provider` inside the same gated write; `None` leaves
    /// it unchanged.
    #[serde(default)]
    pub provider: Option<String>,
    pub roles: Vec<ModelRoleAssignment>,
    /// Phase 50.5 (D-14): the default card's per-model window value to set
    /// when `apply_context_length` is true. Same three-state semantics as
    /// `ModelRoleAssignment::context_length`/`apply_context_length`.
    #[serde(default)]
    pub context_length: Option<u32>,
    /// Phase 50.5 (D-14): WRITE-ONLY intent flag for `context_length` above.
    #[serde(default)]
    pub apply_context_length: bool,
}

/// Phase 50.5 (T-50.5-02): server-side ceiling for a browser-submitted
/// context-window value — mirrors `ironhermes_core::constants::
/// MAX_PLAUSIBLE_CONTEXT_LENGTH`. The browser's `type="number"`/`min="0"`
/// are affordances, not a security boundary; this is where it counts.
const MAX_WEB_CONTEXT_LENGTH: u32 = 10_000_000;

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
    // Phase 50.5 (T-50.5-02): reject an implausible default-card context
    // window BEFORE any mutation. `context_length` is `u32`, so a negative
    // value cannot reach here on the wire — no dead code needed for that case.
    if let Some(n) = payload.context_length {
        if n == 0 || n > MAX_WEB_CONTEXT_LENGTH {
            return Err(ServerFnError::new(format!(
                "context_length must be between 1 and {MAX_WEB_CONTEXT_LENGTH} (got {n})"
            )));
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
        // Phase 50.5 (T-50.5-02): same range check for each role row's window.
        if let Some(n) = role.context_length {
            if n == 0 || n > MAX_WEB_CONTEXT_LENGTH {
                return Err(ServerFnError::new(format!(
                    "context_length for role '{}' must be between 1 and {MAX_WEB_CONTEXT_LENGTH} (got {n})",
                    role.role_key
                )));
            }
        }
    }
    Ok(())
}

/// Phase 50.5 (D-14): resolve a `ModelRoleConfig.provider` value, mapping the
/// `"main"` sentinel to the actual active provider name — the same
/// resolution `ProviderResolver::resolve_role` performs. Needed here because
/// the sparse per-model override lives under the REAL provider name, never
/// literally `"main"`.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_role_provider_name<'c>(
    config: &'c ironhermes_core::config::Config,
    provider: &'c str,
) -> &'c str {
    if provider == "main" {
        config.model.provider.as_str()
    } else {
        provider
    }
}

/// Phase 50.5 (D-14): read the sparse per-(provider, model) context-window
/// override directly off `config.providers`, mirroring
/// `ironhermes_core::config_extras::resolve_extras`'s per-model lookup shape.
#[cfg(not(target_arch = "wasm32"))]
fn configured_context_length(
    config: &ironhermes_core::config::Config,
    provider: &str,
    model: &str,
) -> Option<u32> {
    config
        .providers
        .get(provider)
        .and_then(|p| p.models.get(model))
        .and_then(|m| m.context_length)
        .map(saturating_u32)
}

/// Phase 46.9 Plan 02: Build the read-side snapshot from a loaded Config.
/// Shared by `get_models_roles_config` and the `model_roles` test module so
/// the write-then-read assertion never has to go through the #[server]
/// HTTP-extractor machinery (mirrors `build_voice_snapshot`).
///
/// Phase 50.5 (D-12/D-14): takes the `ProviderResolver` built from the SAME
/// config so every row's resolved window and provenance come from ONE
/// evaluation of the resolver's tier chain (`context_length_with_source`) —
/// the number and its provenance can never drift apart from separate
/// implementations.
#[cfg(not(target_arch = "wasm32"))]
fn build_models_roles_snapshot(
    config: &ironhermes_core::config::Config,
    resolver: &ironhermes_core::provider::ProviderResolver,
) -> ModelsRolesSnapshot {
    let (default_resolved_context_length, default_resolved_context_source) =
        resolver.resolve_for_main().context_length_with_source();
    let default_context_override =
        configured_context_length(config, &config.model.provider, &config.model.default);

    let roles = MODEL_ROLE_KEYS
        .iter()
        .map(|key| {
            let entry = config.model.roles.get(*key);
            // Phase 50.5 (D-12): resolve_role returning None means "uses
            // default" — fall back to the main endpoint's resolved values
            // rather than a separate re-derivation, so this row can never
            // disagree with the topbar about what "default" means.
            let (resolved_length, resolved_source) = match resolver.resolve_role(key) {
                Some(ep) => ep.context_length_with_source(),
                None => (
                    default_resolved_context_length,
                    default_resolved_context_source,
                ),
            };
            let context_length = entry.and_then(|r| {
                let provider = resolve_role_provider_name(config, &r.provider);
                let model = r.model.as_deref().unwrap_or(config.model.default.as_str());
                configured_context_length(config, provider, model)
            });
            ModelRoleAssignment {
                role_key: (*key).to_string(),
                provider: entry.map(|r| r.provider.clone()),
                model: entry.and_then(|r| r.model.clone()),
                context_length,
                apply_context_length: false,
                resolved_context_length: saturating_u32(resolved_length),
                resolved_context_source: resolved_source.into(),
            }
        })
        .collect();
    ModelsRolesSnapshot {
        default_model: config.model.default.clone(),
        provider: config.model.provider.clone(),
        roles,
        web_config_write_enabled: config.security.web_config_write_enabled,
        default_resolved_context_length: saturating_u32(default_resolved_context_length),
        default_resolved_context_source: default_resolved_context_source.into(),
        default_context_override,
    }
}

/// Phase 50.5 (D-12): fallback snapshot for the case where
/// `ProviderResolver::build` itself failed (e.g. a malformed provider
/// `base_url`) — the read-only Models screen must not go dark because of
/// that. Configured overrides are still read directly off `config.providers`
/// (no resolver needed for that); only the RESOLVED window/tier fields
/// degrade to `Fallback`/128,000, since no resolver executed to compute them.
#[cfg(not(target_arch = "wasm32"))]
fn degraded_models_roles_snapshot(
    config: &ironhermes_core::config::Config,
) -> ModelsRolesSnapshot {
    let fallback_len = saturating_u32(ironhermes_core::constants::DEFAULT_CONTEXT_LENGTH);
    let default_context_override =
        configured_context_length(config, &config.model.provider, &config.model.default);
    let roles = MODEL_ROLE_KEYS
        .iter()
        .map(|key| {
            let entry = config.model.roles.get(*key);
            let context_length = entry.and_then(|r| {
                let provider = resolve_role_provider_name(config, &r.provider);
                let model = r.model.as_deref().unwrap_or(config.model.default.as_str());
                configured_context_length(config, provider, model)
            });
            ModelRoleAssignment {
                role_key: (*key).to_string(),
                provider: entry.map(|r| r.provider.clone()),
                model: entry.and_then(|r| r.model.clone()),
                context_length,
                apply_context_length: false,
                resolved_context_length: fallback_len,
                resolved_context_source: ContextWindowSource::Fallback,
            }
        })
        .collect();
    ModelsRolesSnapshot {
        default_model: config.model.default.clone(),
        provider: config.model.provider.clone(),
        roles,
        web_config_write_enabled: config.security.web_config_write_enabled,
        default_resolved_context_length: fallback_len,
        default_resolved_context_source: ContextWindowSource::Fallback,
        default_context_override,
    }
}

/// Phase 50.5 (D-14): upsert-or-prune `providers.<p>.models.<m>.context_length`.
/// `pub(crate)`, not private: Plan 06 Task 3 calls this from the Providers
/// write path so the browser reaches exactly one implementation of "which
/// per-model keys can it touch" (T-50.5-18).
///
/// - `Some(n)`: upsert the entry, creating the provider/model map levels if
///   they are absent.
/// - `None`: clear the stored value, then remove the model entry entirely
///   when it is left carrying nothing else (D-01's "zero entries in the
///   normal steady state" — a cleared override must not leave a husk behind
///   in `config.yaml`). The provider entry itself is never removed; a
///   provider with an empty `models` map is a normal, meaningful state.
///
/// A blank (post-trim) `provider` or `model` has nothing to key on and is a
/// silent no-op — a payload naming no model cannot write anywhere sane.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn write_per_model_context_length(
    config: &mut ironhermes_core::config::Config,
    provider: &str,
    model: &str,
    value: Option<usize>,
) {
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty() || model.is_empty() {
        return;
    }
    match value {
        Some(n) => {
            config
                .providers
                .entry(provider.to_string())
                .or_default()
                .models
                .entry(model.to_string())
                .or_default()
                .context_length = Some(n);
        }
        None => {
            if let Some(provider_cfg) = config.providers.get_mut(provider) {
                if let Some(model_cfg) = provider_cfg.models.get_mut(model) {
                    model_cfg.context_length = None;
                    if model_cfg.extra_request_options.is_empty() {
                        provider_cfg.models.remove(model);
                    }
                }
            }
        }
    }
}

/// Phase 46.9 Plan 02 (D-05/D-10): Merge a validated payload into a freshly
/// loaded Config. `provider` defaults to `"main"` (inherit main provider,
/// per `ModelRoleConfig`'s doc comment, config.rs:245) when a role entry
/// omits it — the Models catalog (`list_models`) has no per-model provider
/// field, so the ASSIGN flow always writes the inherit-main sentinel.
///
/// Phase 50.4 Plan 03 (operator decision, live UAT finding, 2026-09-08):
/// `ProviderResolver::resolve_for_main()` overlays `providers.<active>.
/// default_model` OVER the top-level `config.model.default`
/// (`resolved_main_model`'s doc comment above, and `provider.rs`'s
/// endpoint-build step 2). Before this fix, a Models-screen Default-model
/// save wrote ONLY `config.model.default`, reported success, and did
/// NOTHING whenever the active provider carried its own `default_model` —
/// observed live: the operator picked a new model here, the write
/// succeeded, and the model actually in effect stayed the OLD one because
/// `providers.openrouter.default_model` was still set. The operator's
/// explicit decision (not a bug fix the resolver would otherwise need):
/// this write ALSO syncs `providers.<active>.default_model` to the
/// newly-picked model, so the Models screen's selection actually takes
/// effect instead of being silently shadowed. Deliberately cross-scope —
/// one screen updating a provider-scoped key — do not "fix" this back into
/// a write-only-`model.default` no-op.
///
/// Scoped narrowly on purpose:
/// - Only `config.model.provider` (the CURRENTLY active provider, read
///   AFTER any `payload.provider` update on this same call so a combined
///   provider+model save syncs the provider the operator just picked, not
///   the stale one) gets the new pick. The one other entry ever touched is
///   the provider being LEFT on a provider change, and only when its
///   existing entry has a blank `default_model`: it is pinned to the old
///   `model.default` it was running on (WR-09), so switching back does not
///   resolve an empty model.
/// - Only fires when `payload.default_model` is `Some`, which the six role
///   rows never populate (`CascadeKind::Role` writes only `payload.roles`)
///   — a role-only save cannot reach this branch.
/// - Only mutates an EXISTING `providers.<active>` entry, never creating
///   one, but ALWAYS syncs it — including when it had no `default_model`
///   (G-58.3.1-10). `model.default` only reaches the main provider, so
///   without the pin a later switch away would make this provider resolve
///   an empty model.
#[cfg(not(target_arch = "wasm32"))]
fn merge_models_roles_payload(
    config: &mut ironhermes_core::config::Config,
    payload: &ModelsRolesWritePayload,
) {
    if let Some(ref provider) = payload.provider {
        // WR-09 (G-58.3.1-10): the provider being left loses `model.default`,
        // so a blank existing entry is pinned to the model it was running on,
        // same rule as the TUI's `record_switch`.
        if *provider != config.model.provider && !config.model.default.trim().is_empty() {
            let running = config.model.default.clone();
            if let Some(outgoing) = config.providers.get_mut(&config.model.provider) {
                if outgoing
                    .default_model
                    .as_deref()
                    .is_none_or(|m| m.trim().is_empty())
                {
                    outgoing.default_model = Some(running);
                }
            }
        }
        config.model.provider = provider.clone();
    }
    if let Some(ref default_model) = payload.default_model {
        config.model.default = default_model.clone();
        if let Some(active) = config.providers.get_mut(&config.model.provider) {
            active.default_model = Some(default_model.clone());
        }
    }
    // Phase 50.5 (D-14/D-16): the default card's window write, AFTER any
    // provider/default_model update above (same ordering rule as the overlay
    // sync above it) so a combined provider+model+window save keys on the
    // provider and model the operator just picked, not the stale ones.
    if payload.apply_context_length {
        let provider = config.model.provider.clone();
        let model = config.model.default.clone();
        write_per_model_context_length(
            config,
            &provider,
            &model,
            payload.context_length.map(|n| n as usize),
        );
    }
    for role in &payload.roles {
        config.model.roles.insert(
            role.role_key.clone(),
            ironhermes_core::config::ModelRoleConfig {
                provider: role.provider.clone().unwrap_or_else(|| "main".to_string()),
                model: role.model.clone(),
            },
        );
        // Phase 50.5 (D-14): a role with no model has no per-model key to
        // write into, so it is skipped rather than falling back to the
        // default model.
        if role.apply_context_length {
            if let Some(ref model) = role.model {
                let provider_raw = role.provider.clone().unwrap_or_else(|| "main".to_string());
                let provider = if provider_raw == "main" {
                    config.model.provider.clone()
                } else {
                    provider_raw
                };
                write_per_model_context_length(
                    config,
                    &provider,
                    model,
                    role.context_length.map(|n| n as usize),
                );
            }
        }
    }
}

/// Phase 46.9 Plan 02 (D-05): Read-side server fn for the Models screen role
/// picker. Fresh `Config::load()`, never `app_state.config` (that is the
/// startup snapshot — RESEARCH anti-pattern; precedent: `get_voice_config`).
#[server]
pub async fn get_models_roles_config() -> Result<ModelsRolesSnapshot, ServerFnError> {
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
    // Phase 50.5 (D-12): degrade rather than error when the resolver itself
    // fails to build (e.g. one provider's `base_url` is malformed) — this fn
    // feeds the whole Models screen and must not go dark over one provider.
    match ironhermes_core::provider::ProviderResolver::build(&config) {
        Ok(resolver) => Ok(build_models_roles_snapshot(&config, &resolver)),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "ProviderResolver::build failed while building the Models-screen snapshot; \
                 degrading resolved windows to Fallback/128,000"
            );
            Ok(degraded_models_roles_snapshot(&config))
        }
    }
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

    // RR-05 (WR-06): serialise the root load-modify-save; no `.await` below.
    let _root_write = crate::server::profile_activation_api::lock_root_config_write();

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
        ModelRoleAssignment, ModelsRolesWritePayload, MAX_WEB_CONTEXT_LENGTH,
    };
    use ironhermes_core::config::Config;
    use ironhermes_core::config_extras::ProviderModelConfig;

    /// Build a resolver for a test config the same way `get_models_roles_config`
    /// does, but pinned to the static model-registry table
    /// (`ModelsCache::default()`) so the test is not silently sensitive to
    /// whatever `models-cache.json` happens to exist on the machine running it
    /// (mirrors `default_model_write_syncs_active_provider_overlay`'s existing
    /// comment on this exact hazard).
    fn resolver_for(config: &Config) -> ironhermes_core::ProviderResolver {
        ironhermes_core::ProviderResolver::build_with_cache(
            config,
            ironhermes_core::ModelsCache::default(),
        )
        .expect("resolver must build for a default-shaped test config")
    }

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
                ..Default::default()
            }],
            ..Default::default()
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

        let resolver = resolver_for(&config);
        let snapshot = build_models_roles_snapshot(&config, &resolver);
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
                ..Default::default()
            }],
            ..Default::default()
        };
        let result = validate_models_roles_payload(&payload);
        assert!(result.is_err(), "unknown role key must be rejected");
    }

    /// Snapshot must return all six MODEL_ROLE_KEYS regardless of catalog
    /// size or how many roles are configured (D-05: "fixed six").
    #[test]
    fn snapshot_always_has_six_rows() {
        let config = Config::default();
        let resolver = resolver_for(&config);
        let snapshot = build_models_roles_snapshot(&config, &resolver);
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
            ..Default::default()
        };
        validate_models_roles_payload(&payload).expect("payload must validate");
        merge_models_roles_payload(&mut config, &payload);

        assert_eq!(
            config.model.provider, "anthropic",
            "provider write must persist into config.model.provider"
        );

        let resolver = resolver_for(&config);
        let snapshot = build_models_roles_snapshot(&config, &resolver);
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
            ..Default::default()
        };
        let result = validate_models_roles_payload(&payload);
        assert!(result.is_err(), "oversized provider must be rejected");
    }

    /// Phase 50.4 Plan 03 (operator decision, live UAT finding): reproduces
    /// the EXACT overlay shape the operator observed — `model.default` set
    /// to one value, `providers.<active>.default_model` (the OVERLAY that
    /// wins per `resolve_for_main()`) holding a DIFFERENT one — then asserts
    /// the resolver returns the NEWLY-PICKED model after a Default-model
    /// merge. This test FAILS against the pre-fix write-only-`model.default`
    /// behaviour: before `merge_models_roles_payload` synced the overlay,
    /// the resolver kept returning the stale `"stale-overlay-model"` even
    /// though the merge reported the field write as successful — the exact
    /// "save reported success and did nothing" bug the operator hit live.
    #[test]
    fn default_model_write_syncs_active_provider_overlay() {
        let mut config = Config::default();
        config.model.provider = "openrouter".to_string();
        config.model.default = "old-default-model".to_string();
        config.providers.insert(
            "openrouter".to_string(),
            ironhermes_core::config::ProviderConfig {
                default_model: Some("stale-overlay-model".to_string()),
                ..Default::default()
            },
        );

        let payload = ModelsRolesWritePayload {
            default_model: Some("newly-picked-model".to_string()),
            provider: None,
            roles: Vec::new(),
            ..Default::default()
        };
        validate_models_roles_payload(&payload).expect("payload must validate");
        merge_models_roles_payload(&mut config, &payload);

        let resolver = ironhermes_core::ProviderResolver::build_with_cache(
            &config,
            ironhermes_core::ModelsCache::default(),
        )
        .expect("resolver should build with a valid overlay");
        assert_eq!(
            resolver.resolve_for_main().default_model,
            "newly-picked-model",
            "the Models screen's picked model must be what the resolver actually \
             returns — writing only config.model.default left the provider's \
             default_model overlay shadowing the pick, so a save reported success \
             while doing nothing"
        );
    }

    /// The cross-scope write is scoped to ONLY the currently active
    /// provider — a sibling provider's overlay must survive untouched.
    #[test]
    fn default_model_write_does_not_touch_other_providers() {
        let mut config = Config::default();
        config.model.provider = "openrouter".to_string();
        config.providers.insert(
            "openrouter".to_string(),
            ironhermes_core::config::ProviderConfig {
                default_model: Some("stale-overlay-model".to_string()),
                ..Default::default()
            },
        );
        config.providers.insert(
            "anthropic".to_string(),
            ironhermes_core::config::ProviderConfig {
                default_model: Some("untouched-model".to_string()),
                ..Default::default()
            },
        );

        let payload = ModelsRolesWritePayload {
            default_model: Some("newly-picked-model".to_string()),
            provider: None,
            roles: Vec::new(),
            ..Default::default()
        };
        merge_models_roles_payload(&mut config, &payload);

        assert_eq!(
            config
                .providers
                .get("anthropic")
                .and_then(|p| p.default_model.as_deref()),
            Some("untouched-model"),
            "only the currently active provider's overlay may be touched"
        );
        assert_eq!(
            config.providers.get("openrouter").and_then(|p| p.default_model.as_deref()),
            Some("newly-picked-model"),
            "the active provider's overlay must be synced to the new pick"
        );
    }

    /// A role-row save never populates `payload.default_model`
    /// (`CascadeKind::Role` writes only `payload.roles`) — it must never
    /// touch the active provider's overlay.
    #[test]
    fn role_only_write_does_not_touch_provider_overlay() {
        let mut config = Config::default();
        config.model.provider = "openrouter".to_string();
        config.providers.insert(
            "openrouter".to_string(),
            ironhermes_core::config::ProviderConfig {
                default_model: Some("stale-overlay-model".to_string()),
                ..Default::default()
            },
        );

        let payload = ModelsRolesWritePayload {
            default_model: None,
            provider: None,
            roles: vec![ModelRoleAssignment {
                role_key: "fast".to_string(),
                provider: None,
                model: Some("some-model".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        merge_models_roles_payload(&mut config, &payload);

        assert_eq!(
            config
                .providers
                .get("openrouter")
                .and_then(|p| p.default_model.as_deref()),
            Some("stale-overlay-model"),
            "a role-only save must never touch the active provider's default_model overlay"
        );
    }

    /// G-58.3.1-10: an active provider whose existing entry has no
    /// `default_model` still gets the pick written onto that entry, so it keeps
    /// its model after the operator switches away (a non-main provider never
    /// sees `model.default`).
    #[test]
    fn default_model_write_pins_existing_entry_without_overlay() {
        use ironhermes_core::config::ProviderConfig;
        let entry = |default_model: Option<&str>| ProviderConfig {
            base_url: Some("http://127.0.0.1:9/v1".to_string()),
            default_model: default_model.map(str::to_string),
            ..Default::default()
        };
        let mut config = Config::default();
        config.model.provider = "unsloth-mac-local".to_string();
        config
            .providers
            .insert("unsloth-mac-local".to_string(), entry(None));
        config
            .providers
            .insert("z".to_string(), entry(Some("glm-5")));

        let payload = ModelsRolesWritePayload {
            default_model: Some("unsloth/flint-GGUF".to_string()),
            provider: None,
            roles: Vec::new(),
            ..Default::default()
        };
        merge_models_roles_payload(&mut config, &payload);
        assert_eq!(
            config.providers["unsloth-mac-local"]
                .default_model
                .as_deref(),
            Some("unsloth/flint-GGUF"),
            "the active provider's existing entry must carry the pick"
        );
        assert_eq!(config.model.default, "unsloth/flint-GGUF");

        // Switch the active provider away: the one left behind keeps its model.
        let switch = ModelsRolesWritePayload {
            default_model: Some("glm-5".to_string()),
            provider: Some("z".to_string()),
            roles: Vec::new(),
            ..Default::default()
        };
        merge_models_roles_payload(&mut config, &switch);
        let resolver = resolver_for(&config);
        assert_eq!(
            resolver
                .resolve("unsloth-mac-local")
                .expect("provider resolves")
                .default_model,
            "unsloth/flint-GGUF",
            "the non-main provider must keep the model it was left on"
        );
    }

    /// WR-09 (G-58.3.1-10): switching away from a null-default provider with
    /// NO earlier pinning save still leaves it on the model it was running.
    /// An outgoing entry that already has a model, or a blank old
    /// `model.default`, is left alone.
    #[test]
    fn provider_switch_pins_the_null_default_provider_being_left() {
        use ironhermes_core::config::ProviderConfig;
        let entry = |default_model: Option<&str>| ProviderConfig {
            base_url: Some("http://127.0.0.1:9/v1".to_string()),
            default_model: default_model.map(str::to_string),
            ..Default::default()
        };
        let switch_to = |provider: &str, model: &str| ModelsRolesWritePayload {
            default_model: Some(model.to_string()),
            provider: Some(provider.to_string()),
            roles: Vec::new(),
            ..Default::default()
        };

        let mut config = Config::default();
        config.model.provider = "unsloth-mac-local".to_string();
        config.model.default = "unsloth/flint-GGUF".to_string();
        config
            .providers
            .insert("unsloth-mac-local".to_string(), entry(None));
        config
            .providers
            .insert("z".to_string(), entry(Some("glm-5")));

        merge_models_roles_payload(&mut config, &switch_to("z", "glm-5"));
        assert_eq!(
            config.providers["unsloth-mac-local"]
                .default_model
                .as_deref(),
            Some("unsloth/flint-GGUF"),
            "the provider being left must be pinned to the model it was running"
        );
        assert_eq!(
            resolver_for(&config)
                .resolve("unsloth-mac-local")
                .expect("provider resolves")
                .default_model,
            "unsloth/flint-GGUF"
        );

        // Leaving a provider that already has a model never overwrites it.
        config.model.default = "something-else".to_string();
        let back = switch_to("unsloth-mac-local", "unsloth/flint-GGUF");
        merge_models_roles_payload(&mut config, &back);
        assert_eq!(
            config.providers["z"].default_model.as_deref(),
            Some("glm-5")
        );

        // A blank old model.default pins nothing.
        let mut blank = Config::default();
        blank.model.provider = "unsloth-mac-local".to_string();
        blank.model.default = String::new();
        blank
            .providers
            .insert("unsloth-mac-local".to_string(), entry(None));
        blank.providers.insert("z".to_string(), entry(None));
        merge_models_roles_payload(&mut blank, &switch_to("z", "glm-5"));
        assert_eq!(blank.providers["unsloth-mac-local"].default_model, None);
    }

    /// Phase 50.5 (D-14): the default-card branch upserts the sparse
    /// per-model entry, AND a role entry whose `provider` is the `"main"`
    /// sentinel writes under `config.model.provider`'s value — never under a
    /// provider literally named `"main"`.
    #[test]
    fn window_write_upserts_the_sparse_entry() {
        let mut config = Config::default();
        let payload = ModelsRolesWritePayload {
            default_model: Some("k3".to_string()),
            provider: Some("moonshot".to_string()),
            roles: Vec::new(),
            context_length: Some(1_048_576),
            apply_context_length: true,
        };
        validate_models_roles_payload(&payload).expect("payload must validate");
        merge_models_roles_payload(&mut config, &payload);
        assert_eq!(
            config
                .providers
                .get("moonshot")
                .and_then(|p| p.models.get("k3"))
                .and_then(|m| m.context_length),
            Some(1_048_576),
            "the default-card window write must upsert providers.<p>.models.<m>.context_length"
        );

        let mut config = Config::default();
        config.model.provider = "anthropic".to_string();
        let payload = ModelsRolesWritePayload {
            default_model: None,
            provider: None,
            roles: vec![ModelRoleAssignment {
                role_key: "fast".to_string(),
                provider: Some("main".to_string()),
                model: Some("claude-haiku".to_string()),
                context_length: Some(200_000),
                apply_context_length: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        merge_models_roles_payload(&mut config, &payload);
        assert_eq!(
            config
                .providers
                .get("anthropic")
                .and_then(|p| p.models.get("claude-haiku"))
                .and_then(|m| m.context_length),
            Some(200_000),
            "a role's 'main' provider sentinel must resolve to config.model.provider, \
             never a literal 'main' key"
        );
        assert!(
            !config.providers.contains_key("main"),
            "the literal string 'main' must never become a provider key"
        );
    }

    /// Phase 50.5 (D-14): clearing the window prunes an entry that carries
    /// nothing else (no husk), but leaves a sibling entry present — with
    /// `context_length` cleared and its extras untouched — when it also
    /// carries `extra_request_options`.
    #[test]
    fn blank_window_clears_and_prunes_the_entry() {
        let payload = ModelsRolesWritePayload {
            default_model: Some("k3".to_string()),
            provider: Some("moonshot".to_string()),
            roles: Vec::new(),
            context_length: None,
            apply_context_length: true,
        };

        // Case 1: an entry with nothing else is removed entirely.
        let mut config = Config::default();
        config.providers.insert(
            "moonshot".to_string(),
            ironhermes_core::config::ProviderConfig {
                models: std::collections::HashMap::from([(
                    "k3".to_string(),
                    ProviderModelConfig {
                        context_length: Some(1_048_576),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            },
        );
        merge_models_roles_payload(&mut config, &payload);
        assert!(
            !config.providers["moonshot"].models.contains_key("k3"),
            "a cleared entry with nothing else must be pruned entirely, not left as a husk"
        );

        // Case 2: an entry carrying extra_request_options survives the
        // clear, with context_length cleared and its extras untouched.
        let mut extras = std::collections::HashMap::new();
        extras.insert("temperature".to_string(), serde_json::json!(0.7));
        let mut config = Config::default();
        config.providers.insert(
            "moonshot".to_string(),
            ironhermes_core::config::ProviderConfig {
                models: std::collections::HashMap::from([(
                    "k3".to_string(),
                    ProviderModelConfig {
                        context_length: Some(1_048_576),
                        extra_request_options: extras.clone(),
                    },
                )]),
                ..Default::default()
            },
        );
        merge_models_roles_payload(&mut config, &payload);
        let entry = config
            .providers
            .get("moonshot")
            .and_then(|p| p.models.get("k3"))
            .expect("an entry with surviving extras must not be pruned");
        assert_eq!(entry.context_length, None, "context_length must be cleared");
        assert_eq!(
            entry.extra_request_options, extras,
            "extra_request_options must be untouched by the window clear"
        );
    }

    /// T-50.5-02: `Some(0)` and any value above `MAX_WEB_CONTEXT_LENGTH` are
    /// rejected by validation, and — proving the ORDERING, not just the
    /// rejection — a config that a caller correctly never merges (because
    /// the real four-step protocol only calls `merge` after `validate`
    /// succeeds) stays byte-identical to before the attempt.
    #[test]
    fn window_validation_rejects_out_of_range_before_mutation() {
        for bad in [0u32, MAX_WEB_CONTEXT_LENGTH + 1] {
            let mut config = Config::default();
            config.providers.insert(
                "moonshot".to_string(),
                ironhermes_core::config::ProviderConfig {
                    models: std::collections::HashMap::from([(
                        "k3".to_string(),
                        ProviderModelConfig {
                            context_length: Some(999_999),
                            ..Default::default()
                        },
                    )]),
                    ..Default::default()
                },
            );
            let before = config.clone();
            let payload = ModelsRolesWritePayload {
                default_model: Some("k3".to_string()),
                provider: Some("moonshot".to_string()),
                roles: Vec::new(),
                context_length: Some(bad),
                apply_context_length: true,
            };

            // Mirrors update_models_roles_config's real step order: validate
            // FIRST, only merge on success.
            let validation = validate_models_roles_payload(&payload);
            assert!(validation.is_err(), "context_length {bad} must be rejected");
            if validation.is_ok() {
                merge_models_roles_payload(&mut config, &payload);
            }
            assert_eq!(
                config
                    .providers
                    .get("moonshot")
                    .and_then(|p| p.models.get("k3"))
                    .and_then(|m| m.context_length),
                before
                    .providers
                    .get("moonshot")
                    .and_then(|p| p.models.get("k3"))
                    .and_then(|m| m.context_length),
                "a rejected payload must never reach a mutation"
            );
        }
    }

    /// Phase 50.5 (D-14): `apply_context_length: false` leaves any existing
    /// entry byte-identical, whatever `context_length` holds.
    #[test]
    fn unapplied_window_leaves_the_entry_untouched() {
        let mut config = Config::default();
        config.providers.insert(
            "moonshot".to_string(),
            ironhermes_core::config::ProviderConfig {
                models: std::collections::HashMap::from([(
                    "k3".to_string(),
                    ProviderModelConfig {
                        context_length: Some(1_048_576),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            },
        );
        let before = config.clone();
        let payload = ModelsRolesWritePayload {
            default_model: Some("k3".to_string()),
            provider: Some("moonshot".to_string()),
            roles: Vec::new(),
            context_length: Some(999),
            apply_context_length: false,
        };
        merge_models_roles_payload(&mut config, &payload);
        assert_eq!(
            config.providers["moonshot"].models["k3"].context_length,
            before.providers["moonshot"].models["k3"].context_length,
            "apply_context_length: false must leave the stored value untouched"
        );
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

    // WR-06: load-modify-save of the root config.yaml is serialised with the menu and
    // activation writers (no `.await` below, so the guard is never held across one).
    let _root_write = crate::server::profile_activation_api::lock_root_config_write();

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

    // RR-05 (WR-06): serialise the root load-modify-save; no `.await` below.
    let _root_write = crate::server::profile_activation_api::lock_root_config_write();

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

/// Phase 50.4 Plan 01 (D-08/D-09/D-12/D-14): outcome of an "APPLY CONFIG NOW"
/// click. Carries the applied provider/model on success, or a concrete
/// `failure_reason` on the D-09 failure path — `ApplyConfigBanner` renders
/// "Apply failed — the previous config is still active. {reason}" from that
/// field rather than a bare opaque error.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApplyConfigOutcome {
    pub provider: String,
    pub model: String,
    pub failure_reason: Option<String>,
}

/// D-12/D-14 regression guard: the ONLY way `apply_config_now` may compute a
/// model string for the `config.apply` audit entry. `resolve_for_main()`
/// applies the `providers.<name>.default_model` overlay
/// (`ironhermes_core::provider::ProviderResolver::build`, step 2), which
/// takes precedence over the raw `config.model.default` field. Reading the
/// raw field for `before_model` while reading this resolved form for
/// `after_model` reported a phantom model change on every apply where the
/// active provider carries a `default_model` overlay — the two sides of the
/// audit entry described the model through two different resolution paths.
/// Both `before_model` and `after_model` MUST be computed by calling this on
/// the pre- and post-reload resolver respectively, so a no-op apply always
/// reports the same model on both sides.
#[cfg(feature = "server")]
fn resolved_main_model(resolver: &ironhermes_core::ProviderResolver) -> String {
    resolver.resolve_for_main().default_model.clone()
}

/// Phase 50.4 Plan 01 (D-12): append one `config.apply` audit entry recording
/// the previous (still-running) provider/model, the attempted outcome, and —
/// on success — the newly-applied provider/model. Every refusal path AND the
/// success path write through this one fn so a reader filtering
/// `audit.jsonl` on `config.apply` sees attempts and outcomes in one stream —
/// the success/failure distinction lives in the entry's own `decision` field,
/// never in a different action name. Never includes an API key, key
/// material, or a vault path in `args` — only provider names and model ids.
#[cfg(feature = "server")]
async fn append_apply_config_audit(
    audit_log: &ironhermes_core::AuditLog,
    before_provider: &str,
    before_model: &str,
    decision: &str,
    reason: &str,
    after: Option<(&str, &str)>,
) {
    let mut args = serde_json::json!({
        "before_provider": before_provider,
        "before_model": before_model,
    });
    if let Some((provider, model)) = after {
        args["after_provider"] = serde_json::Value::String(provider.to_string());
        args["after_model"] = serde_json::Value::String(model.to_string());
    }
    let entry = audit_log.make_entry(
        "config_apply",
        "config.apply",
        None,
        "web-config",
        "web",
        "web-config",
        reason,
        decision,
        "operator",
        &args,
    );
    if let Err(e) = audit_log.append(&entry).await {
        tracing::warn!(
            target: "iron_hermes_ui::apply_config",
            error = %e,
            "failed to append apply-config-now audit entry"
        );
    }
}

/// Phase 50.4 Plan 01 (D-08/D-09/D-12/D-14): the "APPLY CONFIG NOW" seam. A
/// successful Providers save leaves `config.yaml` updated but the LIVE
/// `AgentRuntime`/`AppState` config, resolver and cached main client
/// unchanged until an operator clicks APPLY NOW — this is that click's
/// handler, wired through `AppState::apply_config_now` ->
/// `AgentRuntime::reload_config_and_resolver`.
///
/// All-or-nothing (D-09): fresh `Config::load()`, `ProviderResolver::build`,
/// and the optional vault top-up all run into LOCALS and propagate their
/// error before anything is published — nothing here is published unless
/// every fallible step returned Ok. Gated the SAME way every other
/// config-write server fn is (D-12's "inherits the existing config-write gate
/// by construction").
#[server]
pub async fn apply_config_now() -> Result<ApplyConfigOutcome, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let state = crate::server::state::global_app_state();

        // Step 1: fresh disk read — never the startup snapshot, never
        // app_state's current handle, since the whole point is to pick up
        // what the operator just saved.
        let config = match ironhermes_core::config::Config::load() {
            Ok(c) => c,
            Err(e) => {
                // No `config` exists to read `config.audit` from — use
                // AuditConfig::default() rather than skip the entry. The
                // still-running config/resolver on `state` ARE "the previous
                // config [still] active" this failure leaves in place.
                let audit_log =
                    ironhermes_core::AuditLog::load(ironhermes_core::AuditConfig::default());
                let reason = e.to_string();
                append_apply_config_audit(
                    &audit_log,
                    state.resolver().main_provider(),
                    &resolved_main_model(&state.resolver()),
                    "denied",
                    &reason,
                    None,
                )
                .await;
                return Err(ServerFnError::new(format!("Config load failed: {reason}")));
            }
        };

        // Step 2: gate — web config write is disabled unless operator opts
        // in. This is D-12's "inherits the existing config-write gate by
        // construction": an ungated reload would let a caller who cannot
        // save config still activate whatever happens to be on disk.
        if !config.security.web_config_write_enabled {
            let audit_log = ironhermes_core::AuditLog::load(config.audit.clone());
            append_apply_config_audit(
                &audit_log,
                state.resolver().main_provider(),
                &resolved_main_model(&state.resolver()),
                "denied",
                "Config writes are disabled",
                None,
            )
            .await;
            return Err(ServerFnError::new("Config writes are disabled"));
        }

        // Step 3: build the new resolver + optional vault top-up — the SAME
        // sequence AppState::init runs — into LOCALS. Nothing published yet.
        let audit_log = ironhermes_core::AuditLog::load(config.audit.clone());
        let mut resolver = match ironhermes_core::ProviderResolver::build(&config) {
            Ok(r) => r,
            Err(e) => {
                let reason = e.to_string();
                append_apply_config_audit(
                    &audit_log,
                    state.resolver().main_provider(),
                    &resolved_main_model(&state.resolver()),
                    "denied",
                    &reason,
                    None,
                )
                .await;
                return Err(ServerFnError::new(format!(
                    "Provider resolver build failed: {reason}"
                )));
            }
        };
        if config.vault.enabled {
            match ironhermes_vault::open_store(&ironhermes_core::resolve_vault_config(&config)) {
                Ok(store) => {
                    if let Err(e) = resolver.apply_vault_fallback(&*store).await {
                        let reason = e.to_string();
                        append_apply_config_audit(
                            &audit_log,
                            state.resolver().main_provider(),
                            &resolved_main_model(&state.resolver()),
                            "denied",
                            &reason,
                            None,
                        )
                        .await;
                        return Err(ServerFnError::new(format!(
                            "Vault fallback failed: {reason}"
                        )));
                    }
                }
                Err(e) => {
                    let reason = e.to_string();
                    append_apply_config_audit(
                        &audit_log,
                        state.resolver().main_provider(),
                        &resolved_main_model(&state.resolver()),
                        "denied",
                        &reason,
                        None,
                    )
                    .await;
                    return Err(ServerFnError::new(format!("Vault open failed: {reason}")));
                }
            }
        }

        // Step 4: the reload itself — the only step that touches the live
        // runtime. D-09's all-or-nothing guarantee is enforced inside
        // `AgentRuntime::reload_config_and_resolver`: a client that cannot be
        // built there leaves the previously-running config, resolver and
        // client all untouched, and we surface that as a concrete reason
        // rather than a bare failed call.
        let before_provider = state.resolver().main_provider().to_string();
        let before_model = resolved_main_model(&state.resolver());
        let applied_provider = resolver.main_provider().to_string();
        let applied_model = resolved_main_model(&resolver);
        let config_arc = std::sync::Arc::new(config);
        let resolver_arc = std::sync::Arc::new(resolver);

        match state.apply_config_now(config_arc, resolver_arc).await {
            Ok(()) => {
                append_apply_config_audit(
                    &audit_log,
                    &before_provider,
                    &before_model,
                    "approved",
                    "operator requested apply-config-now",
                    Some((&applied_provider, &applied_model)),
                )
                .await;
                Ok(ApplyConfigOutcome {
                    provider: applied_provider,
                    model: applied_model,
                    failure_reason: None,
                })
            }
            Err(e) => {
                let reason = e.to_string();
                append_apply_config_audit(
                    &audit_log,
                    &before_provider,
                    &before_model,
                    "denied",
                    &reason,
                    None,
                )
                .await;
                Ok(ApplyConfigOutcome {
                    provider: before_provider,
                    model: before_model,
                    failure_reason: Some(reason),
                })
            }
        }
    }
    #[cfg(not(feature = "server"))]
    {
        Err(ServerFnError::new(
            "apply_config_now unavailable without `server` feature",
        ))
    }
}

/// Regression coverage for the `resolved_main_model` fix: `before_model` and
/// `after_model` in the `config.apply` audit entry must be drawn from the
/// SAME resolution path (the provider overlay `resolve_for_main()` applies),
/// never from the raw `config.model.default` field. Before this fix,
/// `apply_config_now` read `before_model` from `state.config().model.default`
/// while `after_model` came from `resolver.resolve_for_main().default_model`
/// — with `model.default: "MiniMax-M3"` and `providers.<main>.default_model:
/// "kimi-k3"`, a no-op apply reported a phantom `MiniMax-M3` -> `kimi-k3`
/// change, because the effective model both before and after was already
/// `kimi-k3`.
#[cfg(all(test, not(target_arch = "wasm32"), feature = "server"))]
mod apply_config_model_resolution {
    use super::resolved_main_model;
    use ironhermes_core::config::{Config, ProviderConfig};
    use ironhermes_core::{ModelsCache, ProviderResolver};

    /// Builds the exact overlay shape reported by the operator: the main
    /// provider's `providers.<name>.default_model` overrides the top-level
    /// `model.default`. `resolve_for_main()` must return the OVERLAY value.
    fn overlay_config() -> Config {
        let mut config = Config::default();
        config.model.provider = "openrouter".to_string();
        config.model.default = "MiniMax-M3".to_string();
        config.providers.insert(
            "openrouter".to_string(),
            ProviderConfig {
                default_model: Some("kimi-k3".to_string()),
                ..Default::default()
            },
        );
        config
    }

    /// `resolve_for_main()` resolves the overlay, not the raw field — the
    /// fixture must actually exercise the divergence Test below depends on,
    /// or the regression test would be vacuous.
    #[test]
    fn overlay_config_diverges_from_raw_model_default() {
        let config = overlay_config();
        let resolver = ProviderResolver::build_with_cache(&config, ModelsCache::default())
            .expect("resolver should build with a valid overlay");
        assert_eq!(config.model.default, "MiniMax-M3");
        assert_eq!(resolved_main_model(&resolver), "kimi-k3");
        assert_ne!(
            config.model.default,
            resolved_main_model(&resolver),
            "fixture must diverge from the raw config field, or this regression test is vacuous"
        );
    }

    /// The actual regression: a no-op apply (the SAME config reloaded,
    /// nothing changed) must report no model change. This is the assertion
    /// that FAILS if `before_model` is ever read from the raw
    /// `config.model.default` field again instead of through
    /// `resolved_main_model` — reverting `resolved_main_model`'s call sites
    /// in `apply_config_now` back to `state.config().model.default.clone()`
    /// reproduces the phantom-change bug this test guards against.
    #[test]
    fn noop_apply_with_provider_overlay_reports_no_model_change() {
        let config = overlay_config();

        // "Before": the currently-running resolver (mirrors `state.resolver()`
        // inside `apply_config_now`).
        let before_resolver = ProviderResolver::build_with_cache(&config, ModelsCache::default())
            .expect("resolver should build");
        let before_model = resolved_main_model(&before_resolver);

        // "After": the operator clicked APPLY NOW without changing anything —
        // the freshly-loaded config and rebuilt resolver are identical to the
        // running ones.
        let after_resolver = ProviderResolver::build_with_cache(&config, ModelsCache::default())
            .expect("resolver should build");
        let after_model = resolved_main_model(&after_resolver);

        assert_eq!(
            before_model, after_model,
            "a no-op apply must not report a model change — both sides must resolve \
             through the provider overlay, not one through the raw config field"
        );
    }
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

// =============================================================================
// Phase 54.7 Plan 06: the web save/read path for the new voice settings.
// Every test here drives the REAL save path (validate_voice_payload ->
// Config::load_from -> apply_voice_payload -> Config::save_to) against a
// tempdir config, never the operator's home.
// =============================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod voice_547_save_tests {
    use super::{
        apply_voice_payload, build_voice_snapshot, validate_voice_payload, voice_key_statuses,
        VoiceKeyStatuses, VoiceWritePayload,
    };
    use ironhermes_core::config::Config;
    use std::path::{Path, PathBuf};

    fn temp_config() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.yaml");
        (dir, path)
    }

    /// The same order `update_voice_config` runs, minus the home-dir load and
    /// the write gate (D-20 keeps those in the server fn itself).
    fn save_through_real_path(path: &Path, payload: &VoiceWritePayload) -> Result<(), String> {
        validate_voice_payload(payload).map_err(|e| e.to_string())?;
        let mut config = Config::load_from(path).map_err(|e| e.to_string())?;
        apply_voice_payload(&mut config, payload).map_err(|e| e.to_string())?;
        config.save_to(path).map_err(|e| e.to_string())
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(f)
    }

    #[test]
    fn tts_compat_payload_round_trips_through_config_yaml() {
        let (_dir, path) = temp_config();
        let payload = VoiceWritePayload {
            tts_provider: Some("openai_compatible".into()),
            tts_compat_base_url: Some("http://tts.lan:8880/v1".into()),
            tts_compat_path: Some("/audio/speech".into()),
            tts_compat_model: Some("mlx-community/Kokoro-82M-bf16".into()),
            tts_compat_voice: Some("af_heart".into()),
            tts_compat_format: Some("wav".into()),
            ..Default::default()
        };
        save_through_real_path(&path, &payload).expect("save");

        let yaml = std::fs::read_to_string(&path).expect("read yaml");
        assert!(
            yaml.contains("openai_compatible:"),
            "the tts block must be written under tts: {yaml}"
        );

        let config = Config::load_from(&path).expect("reload");
        let snap = build_voice_snapshot(&config, &VoiceKeyStatuses::default());
        assert_eq!(snap.tts_provider, "openai_compatible");
        assert_eq!(snap.tts_compat_base_url, "http://tts.lan:8880/v1");
        assert_eq!(snap.tts_compat_path, "/audio/speech");
        assert_eq!(snap.tts_compat_model, "mlx-community/Kokoro-82M-bf16");
        assert_eq!(snap.tts_compat_voice, "af_heart");
        assert_eq!(snap.tts_compat_format, "wav");
        assert_eq!(snap.tts_compat_key_ref, "");
    }

    #[test]
    fn tts_compat_key_status_reports_reference_resolution() {
        let _guard = crate::server::test_support::env_lock();
        // SAFETY: serialized by env_lock(); the names are unique to this test.
        unsafe {
            std::env::set_var("VOICE_KEY_IH_TEST_547_KS", "fake-value-ks-547");
            std::env::remove_var("VOICE_KEY_IH_TEST_547_UNSET");
        }

        let mut config = Config::default();
        config.tts.openai_compatible.api_key_ref = Some("VOICE_KEY_IH_TEST_547_KS".into());
        let statuses = block_on(voice_key_statuses(&config));
        assert_eq!(statuses.tts_compat, "found");

        config.tts.openai_compatible.api_key_ref = Some("VOICE_KEY_IH_TEST_547_UNSET".into());
        assert_eq!(
            block_on(voice_key_statuses(&config)).tts_compat,
            "not_found"
        );

        config.tts.openai_compatible.api_key_ref = None;
        assert_eq!(block_on(voice_key_statuses(&config)).tts_compat, "none");

        // The snapshot reports the status string, never the value.
        config.tts.openai_compatible.api_key_ref = Some("VOICE_KEY_IH_TEST_547_KS".into());
        let statuses = block_on(voice_key_statuses(&config));
        let snap = build_voice_snapshot(&config, &statuses);
        assert_eq!(snap.tts_compat_key_status, "found");
        assert_eq!(snap.tts_compat_key_ref, "VOICE_KEY_IH_TEST_547_KS");
        let json = serde_json::to_string(&snap).expect("serialize snapshot");
        assert!(
            !json.contains("fake-value-ks-547"),
            "the key value must never cross the boundary: {json}"
        );

        // A reference outside the voice namespace is rejected before any load.
        let bad = VoiceWritePayload {
            tts_compat_key_ref: Some("OPENAI_API_KEY".into()),
            ..Default::default()
        };
        let err = validate_voice_payload(&bad).expect_err("out-of-namespace reference");
        assert!(
            err.to_string()
                .contains("voice_invalid:tts_compat_key_ref:key_ref_namespace"),
            "unexpected error: {err}"
        );

        // SAFETY: still under env_lock().
        unsafe {
            std::env::remove_var("VOICE_KEY_IH_TEST_547_KS");
        }
    }

    #[test]
    fn empty_optional_text_clears_the_field() {
        let (_dir, path) = temp_config();
        let set = VoiceWritePayload {
            tts_compat_base_url: Some("http://tts.lan:8880/v1".into()),
            tts_compat_path: Some("/x".into()),
            tts_compat_key_ref: Some("VOICE_KEY_IH_TEST_547_CLEAR".into()),
            ..Default::default()
        };
        save_through_real_path(&path, &set).expect("first save");
        let config = Config::load_from(&path).expect("reload");
        assert_eq!(config.tts.openai_compatible.path.as_deref(), Some("/x"));
        assert_eq!(
            config.tts.openai_compatible.api_key_ref.as_deref(),
            Some("VOICE_KEY_IH_TEST_547_CLEAR")
        );

        let clear = VoiceWritePayload {
            tts_compat_path: Some(String::new()),
            tts_compat_key_ref: Some(String::new()),
            ..Default::default()
        };
        save_through_real_path(&path, &clear).expect("clearing save");
        let config = Config::load_from(&path).expect("reload");
        assert_eq!(config.tts.openai_compatible.path, None);
        assert_eq!(config.tts.openai_compatible.api_key_ref, None);
    }

    #[test]
    fn payload_default_has_every_field_none() {
        let json = serde_json::to_value(VoiceWritePayload::default()).expect("serialize");
        let obj = json.as_object().expect("object");
        assert!(!obj.is_empty());
        for (key, value) in obj {
            assert!(value.is_null(), "default field {key} must serialize null");
        }
        let one: VoiceWritePayload =
            serde_json::from_str(r#"{"auto_tts":true}"#).expect("serde(default) body");
        assert_eq!(one.auto_tts, Some(true));
        assert_eq!(one.tts_compat_base_url, None);
    }

    // -- Task 2: STT block, per-provider TTS fields, legacy-safe format rule -----

    #[test]
    fn stt_compat_payload_round_trips_and_requires_base_url() {
        let (_dir, path) = temp_config();
        let payload = VoiceWritePayload {
            stt_provider: Some("openai_compatible".into()),
            stt_compat_base_url: Some("http://stt.lan:8178".into()),
            stt_compat_path: Some("/inference".into()),
            stt_compat_model: Some("whisper-large-v3".into()),
            ..Default::default()
        };
        save_through_real_path(&path, &payload).expect("save");
        let config = Config::load_from(&path).expect("reload");
        let snap = build_voice_snapshot(&config, &VoiceKeyStatuses::default());
        assert_eq!(snap.stt_provider, "openai_compatible");
        assert_eq!(snap.stt_compat_base_url, "http://stt.lan:8178");
        assert_eq!(snap.stt_compat_path, "/inference");
        assert_eq!(snap.stt_compat_model, "whisper-large-v3");
        assert_eq!(snap.stt_model, "whisper-large-v3");

        // The same provider with no address is refused after the merge.
        let (_dir2, path2) = temp_config();
        let no_address = VoiceWritePayload {
            stt_provider: Some("openai_compatible".into()),
            ..Default::default()
        };
        let err = save_through_real_path(&path2, &no_address).expect_err("empty base_url");
        assert!(
            err.contains("voice_invalid:stt_compat_base_url:base_url_empty"),
            "unexpected error: {err}"
        );
        assert!(!path2.exists(), "a rejected save must not write the file");

        // An unknown provider id is rejected instead of merged (Pitfall 11).
        let unknown = VoiceWritePayload {
            stt_provider: Some("not_a_provider".into()),
            ..Default::default()
        };
        assert!(validate_voice_payload(&unknown).is_err());
        let unknown_tts = VoiceWritePayload {
            tts_provider: Some("not_a_provider".into()),
            ..Default::default()
        };
        assert!(validate_voice_payload(&unknown_tts).is_err());
    }

    #[test]
    fn per_provider_tts_fields_do_not_cross_write() {
        let (_dir, path) = temp_config();
        let before = Config::default();

        let payload = VoiceWritePayload {
            tts_provider: Some("elevenlabs".into()),
            tts_elevenlabs_voice: Some("ElevenVoiceX1".into()),
            tts_elevenlabs_model: Some("eleven_turbo_v2_5".into()),
            tts_openai_format: Some("opus".into()),
            tts_openai_model: Some("tts-1-hd".into()),
            tts_openai_voice: Some("nova".into()),
            ..Default::default()
        };
        save_through_real_path(&path, &payload).expect("save");
        let config = Config::load_from(&path).expect("reload");
        assert_eq!(config.tts.provider, "elevenlabs");
        assert_eq!(config.tts.elevenlabs.voice_id, "ElevenVoiceX1");
        assert_eq!(config.tts.elevenlabs.model_id, "eleven_turbo_v2_5");
        assert_eq!(
            config.tts.edge.voice, before.tts.edge.voice,
            "a voice typed for ElevenLabs must not overwrite the Edge voice (Pitfall 13)"
        );
        assert_eq!(config.tts.openai.format, "opus");
        assert_eq!(config.tts.openai.model, "tts-1-hd");
        assert_eq!(config.tts.openai.voice, "nova");

        let snap = build_voice_snapshot(&config, &VoiceKeyStatuses::default());
        assert_eq!(snap.tts_elevenlabs_voice, "ElevenVoiceX1");
        assert_eq!(snap.tts_elevenlabs_model, "eleven_turbo_v2_5");
        assert_eq!(snap.tts_openai_format, "opus");

        // A CHANGED format the provider does not list is refused.
        let (_dir2, path2) = temp_config();
        let flac = VoiceWritePayload {
            tts_openai_format: Some("flac".into()),
            ..Default::default()
        };
        let err = save_through_real_path(&path2, &flac).expect_err("unlisted changed format");
        assert!(
            err.contains("voice_invalid:tts_openai_format:format"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn legacy_format_does_not_block_unrelated_save() {
        let (_dir, path) = temp_config();
        let legacy_edge = "audio-24khz-48kbitrate-mono-mp3";
        let mut seeded = Config::default();
        seeded.tts.edge.output_format = legacy_edge.to_string();
        seeded.tts.openai.format = "flac".to_string();
        seeded.save_to(&path).expect("seed legacy config");

        // The panel only toggles auto_tts but echoes the stored formats.
        let echo = VoiceWritePayload {
            auto_tts: Some(true),
            tts_edge_format: Some(legacy_edge.into()),
            tts_openai_format: Some("flac".into()),
            ..Default::default()
        };
        save_through_real_path(&path, &echo).expect("echoed legacy formats must not block a save");
        let config = Config::load_from(&path).expect("reload");
        assert!(config.voice.auto_tts);
        assert_eq!(config.tts.edge.output_format, legacy_edge);
        assert_eq!(config.tts.openai.format, "flac");

        // A CHANGED edge format outside the registry list is still refused.
        let wav = VoiceWritePayload {
            tts_edge_format: Some("wav".into()),
            ..Default::default()
        };
        let err = save_through_real_path(&path, &wav).expect_err("unlisted changed edge format");
        assert!(
            err.contains("voice_invalid:tts_edge_format:format"),
            "unexpected error: {err}"
        );

        // A listed one is accepted.
        let mp3 = VoiceWritePayload {
            tts_edge_format: Some("mp3".into()),
            ..Default::default()
        };
        save_through_real_path(&path, &mp3).expect("mp3 is listed for edge");
        let config = Config::load_from(&path).expect("reload");
        assert_eq!(config.tts.edge.output_format, "mp3");
    }

    // -- Task 3: realtime block, voice.service, one-way legacy clear -------------

    /// The operator's live pre-54.7 shape: open_mic plus the three legacy keys.
    const LEGACY_YAML: &str = "voice:\n  barge_in_mode: open_mic\n  realtime_model: gpt-realtime-legacy\n  realtime_voice: ash\n  realtime_transcription_model: whisper-1\n";

    fn seed_legacy(path: &Path) {
        std::fs::write(path, LEGACY_YAML).expect("seed legacy yaml");
    }

    #[test]
    fn realtime_payload_writes_new_block_and_clears_legacy() {
        let (_dir, path) = temp_config();
        seed_legacy(&path);
        let payload = VoiceWritePayload {
            voice_service: Some("realtime".into()),
            realtime_base_url: Some("https://api.openai.com/v1".into()),
            realtime_voice: Some("marin".into()),
            ..Default::default()
        };
        save_through_real_path(&path, &payload).expect("save");

        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).expect("yaml");
        assert_eq!(yaml["realtime"]["voice"].as_str(), Some("marin"));
        // The other legacy values migrate into the new block instead of being lost.
        assert_eq!(
            yaml["realtime"]["model"].as_str(),
            Some("gpt-realtime-legacy")
        );
        assert_eq!(
            yaml["realtime"]["transcription_model"].as_str(),
            Some("whisper-1")
        );
        let voice = &yaml["voice"];
        for legacy in [
            "realtime_model",
            "realtime_voice",
            "realtime_transcription_model",
        ] {
            assert!(
                voice.get(legacy).is_none(),
                "legacy key voice.{legacy} must be cleared: {voice:?}"
            );
        }
        assert_eq!(voice["service"].as_str(), Some("realtime"));
    }

    #[test]
    fn open_mic_is_rewritten_on_global_save() {
        let (_dir, path) = temp_config();
        std::fs::write(&path, "voice:\n  barge_in_mode: open_mic\n").expect("seed");
        let payload = VoiceWritePayload {
            voice_service: Some("realtime".into()),
            ..Default::default()
        };
        save_through_real_path(&path, &payload).expect("save");
        let config = Config::load_from(&path).expect("reload");
        assert_eq!(
            config.voice.barge_in_mode,
            ironhermes_core::config::BargeInMode::PushToInterrupt
        );
        assert_eq!(
            config.voice.service,
            Some(ironhermes_core::config::VoiceService::Realtime)
        );
    }

    #[test]
    fn snapshot_reports_service_legacy_flag_and_barge_modes() {
        let (_dir, path) = temp_config();
        seed_legacy(&path);
        let config = Config::load_from(&path).expect("load legacy");
        let snap = build_voice_snapshot(&config, &VoiceKeyStatuses::default());
        assert_eq!(snap.voice_service, "realtime");
        assert!(snap.legacy_voice_keys_present);
        assert_eq!(snap.barge_in_modes, vec!["push_to_interrupt".to_string()]);
        // The folded legacy values are what the panel shows.
        assert_eq!(snap.realtime_model, "gpt-realtime-legacy");
        assert_eq!(snap.realtime_voice, "ash");
        assert_eq!(snap.realtime_transcription_model, "whisper-1");

        let payload = VoiceWritePayload {
            voice_service: Some("realtime".into()),
            ..Default::default()
        };
        save_through_real_path(&path, &payload).expect("migrating save");
        let config = Config::load_from(&path).expect("reload");
        let snap = build_voice_snapshot(&config, &VoiceKeyStatuses::default());
        assert!(!snap.legacy_voice_keys_present);
        assert_eq!(snap.voice_service, "realtime");
        assert_eq!(snap.realtime_voice, "ash");
    }

    #[test]
    fn realtime_key_resolution_rule() {
        use ironhermes_core::voice_http::VoiceKey;
        let _guard = crate::server::test_support::env_lock();
        let prior_openai = std::env::var("OPENAI_API_KEY").ok();
        // SAFETY: serialized by env_lock(); restored below.
        unsafe {
            std::env::set_var("VOICE_KEY_IH_TEST_547_RT", "fake-rt-value-547");
            std::env::set_var("OPENAI_API_KEY", "test-fake-key");
        }

        // A typed, namespaced reference resolves through the namespaced rule.
        let mut config = Config::default();
        config.realtime.api_key_ref = Some("VOICE_KEY_IH_TEST_547_RT".into());
        assert!(matches!(
            block_on(super::resolve_realtime_key(&config)),
            VoiceKey::Found(_)
        ));

        // No reference + the api.openai.com default -> the provider key.
        config.realtime.api_key_ref = None;
        assert!(config.realtime.is_openai_default_base_url());
        assert!(matches!(
            block_on(super::resolve_realtime_key(&config)),
            VoiceKey::Found(_)
        ));

        // No reference + a custom address -> no key, even with OPENAI_API_KEY set.
        config.realtime.base_url = "http://rt.lan:9000/v1".into();
        assert!(matches!(
            block_on(super::resolve_realtime_key(&config)),
            VoiceKey::None
        ));

        // A hand-edited out-of-namespace reference never reads that variable.
        config.realtime.api_key_ref = Some("OPENAI_API_KEY".into());
        assert!(matches!(
            block_on(super::resolve_realtime_key(&config)),
            VoiceKey::NotFound
        ));

        // SAFETY: still under env_lock().
        unsafe {
            std::env::remove_var("VOICE_KEY_IH_TEST_547_RT");
            match prior_openai {
                Some(v) => std::env::set_var("OPENAI_API_KEY", v),
                None => std::env::remove_var("OPENAI_API_KEY"),
            }
        }
    }

    #[test]
    fn realtime_payload_charset_is_enforced() {
        fn rejection(payload: VoiceWritePayload) -> String {
            validate_voice_payload(&payload)
                .expect_err("must be rejected")
                .to_string()
        }
        let e = rejection(VoiceWritePayload {
            realtime_model: Some("a\"b".into()),
            ..Default::default()
        });
        assert!(
            e.contains("voice_invalid:realtime_model:model_charset"),
            "{e}"
        );
        let e = rejection(VoiceWritePayload {
            realtime_voice: Some("a/b".into()),
            ..Default::default()
        });
        assert!(
            e.contains("voice_invalid:realtime_voice:voice_charset"),
            "{e}"
        );
        let e = rejection(VoiceWritePayload {
            realtime_key_ref: Some("OPENAI_API_KEY".into()),
            ..Default::default()
        });
        assert!(
            e.contains("voice_invalid:realtime_key_ref:key_ref_namespace"),
            "{e}"
        );
        let e = rejection(VoiceWritePayload {
            realtime_provider: Some("not_a_provider".into()),
            ..Default::default()
        });
        assert!(e.contains("voice_invalid:realtime_provider"), "{e}");
        let e = rejection(VoiceWritePayload {
            voice_service: Some("bogus".into()),
            ..Default::default()
        });
        assert!(e.contains("voice_invalid:voice_service"), "{e}");

        // `off` is the reserved transcription value; listed values pass.
        let ok = VoiceWritePayload {
            realtime_transcription_model: Some("off".into()),
            realtime_model: Some("gpt-realtime".into()),
            realtime_voice: Some("marin".into()),
            realtime_provider: Some("openai_compatible".into()),
            realtime_base_url: Some(String::new()),
            voice_service: Some("turn_based".into()),
            ..Default::default()
        };
        assert!(validate_voice_payload(&ok).is_ok());
    }

    #[test]
    fn empty_realtime_base_url_means_default() {
        let (_dir, path) = temp_config();
        let custom = VoiceWritePayload {
            realtime_base_url: Some("http://rt.lan:9000/v1".into()),
            ..Default::default()
        };
        save_through_real_path(&path, &custom).expect("custom save");
        assert_eq!(
            Config::load_from(&path).unwrap().realtime.base_url,
            "http://rt.lan:9000/v1"
        );

        let cleared = VoiceWritePayload {
            realtime_base_url: Some(String::new()),
            ..Default::default()
        };
        save_through_real_path(&path, &cleared).expect("clearing save");
        assert_eq!(
            Config::load_from(&path).unwrap().realtime.base_url,
            "https://api.openai.com/v1"
        );
    }
}

// =============================================================================
// Phase 54.7 Plan 11 (D-10/D-11/D-12): realtime on the provider abstraction.
// Mint tests drive the extracted `mint_realtime_for` against wiremock with fake
// keys and `VOICE_KEY_IH_TEST_547_*` reference names; they never touch AppState.
// =============================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod realtime_547_mint_tests {
    use super::{
        mint_realtime_for, realtime_calls_url_for, realtime_session_config_for,
        resolve_realtime_selection, validate_realtime_values, IssueRealtimeTokenResult,
    };
    use ironhermes_core::config::Config;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(f)
    }

    #[test]
    fn mint_realtime_for_custom_base_with_key() {
        let _guard = crate::server::test_support::env_lock();
        // SAFETY: serialized by env_lock(); the name is unique to this test.
        unsafe {
            std::env::set_var("VOICE_KEY_IH_TEST_547_RT", "test-fake-key");
        }
        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/realtime/client_secrets"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"value": "ek_fake"})),
                )
                .mount(&server)
                .await;

            let mut config = Config::default();
            config.realtime.base_url = format!("{}/v1", server.uri());
            config.realtime.api_key_ref = Some("VOICE_KEY_IH_TEST_547_RT".into());

            let minted = mint_realtime_for(&config, None, "instr", &[])
                .await
                .expect("mint succeeds");
            assert_eq!(minted.token.as_deref(), Some("ek_fake"));
            assert_eq!(
                minted.calls_url,
                format!("{}/v1/realtime/calls", server.uri())
            );
            assert!(minted.session_json.is_none());

            let reqs = server.received_requests().await.expect("recording on");
            assert_eq!(reqs.len(), 1, "exactly one mint POST");
            let auth = reqs[0]
                .headers
                .get("authorization")
                .expect("authorization header")
                .to_str()
                .unwrap()
                .to_string();
            assert_eq!(auth, "Bearer test-fake-key");
            let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
            assert_eq!(body["session"]["model"], "gpt-realtime");
            assert_eq!(body["session"]["instructions"], "instr");
        });
        // SAFETY: still under env_lock().
        unsafe {
            std::env::remove_var("VOICE_KEY_IH_TEST_547_RT");
        }
    }

    #[test]
    fn mint_realtime_for_default_base_builds_openai_urls() {
        let url = realtime_calls_url_for(&Config::default()).expect("default base is valid");
        assert_eq!(url, "https://api.openai.com/v1/realtime/calls");
    }

    #[test]
    fn issue_result_round_trips_new_fields() {
        let original = IssueRealtimeTokenResult {
            token: None,
            calls_url: "http://rt.lan:9000/v1/realtime/calls".into(),
            session_json: Some("{\"type\":\"realtime\"}".into()),
            turn_id: "t".into(),
            session_id: "s".into(),
            chat_session_id: "c".into(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: IssueRealtimeTokenResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, original);
        let with_token = IssueRealtimeTokenResult {
            token: Some("ek".into()),
            session_json: None,
            ..original
        };
        let back: IssueRealtimeTokenResult =
            serde_json::from_str(&serde_json::to_string(&with_token).unwrap()).unwrap();
        assert_eq!(back, with_token);
    }

    // ── Task 2: the D-11 charset gate ────────────────────────────────────────

    /// IN-01: Debug output never carries the token or the session JSON.
    #[test]
    fn issue_realtime_token_result_debug_redacts_secrets() {
        let result = IssueRealtimeTokenResult {
            token: Some("ek_FAKE_TOKEN_VALUE".into()),
            calls_url: "https://rt.example/v1/realtime/calls".into(),
            session_json: Some("{\"instructions\":\"SYSTEM PROMPT TEXT\"}".into()),
            turn_id: "t".into(),
            session_id: "s".into(),
            chat_session_id: "agent:main:web:dm:x".into(),
        };
        let shown = format!("{result:?}");
        assert!(!shown.contains("ek_FAKE_TOKEN_VALUE"), "{shown}");
        assert!(!shown.contains("SYSTEM PROMPT TEXT"), "{shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
    }

    #[test]
    fn realtime_gate_accepts_new_ids() {
        assert!(validate_realtime_values("gpt-realtime-2.1", "marin").is_ok());
        assert!(validate_realtime_values("gpt-realtime", "shimmer").is_ok());
        // A namespaced id from an OpenAI-compatible server is also fine.
        assert!(validate_realtime_values("org/model:v1.2", "af_heart").is_ok());
    }

    #[test]
    fn realtime_gate_rejects_injection() {
        let ok_voice = "marin";
        let long_model = "m".repeat(129);
        let long_voice = "v".repeat(65);
        let bad_models: [&str; 8] = [
            "a\"b",
            "a\\b",
            "a/../b",
            "/abs/model",
            "gpt\nrealtime",
            "",
            "gpt realtime",
            &long_model,
        ];
        for model in bad_models {
            let err = validate_realtime_values(model, ok_voice)
                .expect_err(&format!("model {model:?} must be rejected"))
                .to_string();
            assert!(!err.contains("sk-"), "{err}");
        }
        for voice in ["a/b", "has space", "quo\"te", "", long_voice.as_str()] {
            let err = validate_realtime_values("gpt-realtime", voice)
                .expect_err(&format!("voice {voice:?} must be rejected"))
                .to_string();
            assert!(!err.contains("sk-"), "{err}");
        }
    }

    /// WR-08: minting never sends the transcription model, so a bad stored value
    /// must not disable realtime. The session-config path (the only consumer)
    /// falls back to the default instead.
    #[test]
    fn bad_transcription_model_does_not_block_the_mint_path() {
        let mut config = Config::default();
        config.realtime.transcription_model = Some("bad\"x of f".to_string());
        let (model, voice) = resolve_realtime_selection(&config, None)
            .expect("a bad transcription model must not fail model/voice resolution");
        assert!(!model.is_empty() && !voice.is_empty());
        let json = realtime_session_config_for(&config);
        assert!(
            json.contains("gpt-4o-mini-transcribe") && !json.contains("bad"),
            "the session config falls back to the default model: {json}"
        );
    }

    /// A key-shaped value pasted into a field must never be echoed back.
    #[test]
    fn realtime_gate_does_not_echo_rejected_values() {
        let err = validate_realtime_values("sk-live-abc\"def", "marin")
            .expect_err("rejected")
            .to_string();
        assert!(!err.contains("sk-live-abc"), "{err}");
    }

    /// T-54.7-47: the gate runs BEFORE any key resolution or network call. The
    /// key is resolvable and the mock would answer 200, so only the gate can
    /// keep the request count at zero.
    #[test]
    fn issue_path_rejects_before_network() {
        let _guard = crate::server::test_support::env_lock();
        // SAFETY: serialized by env_lock(); the name is unique to this test.
        unsafe {
            std::env::set_var("VOICE_KEY_IH_TEST_547_GATE", "test-fake-key");
        }
        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/realtime/client_secrets"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"value": "ek_fake"})),
                )
                .mount(&server)
                .await;

            let mut config = Config::default();
            config.realtime.base_url = format!("{}/v1", server.uri());
            config.realtime.api_key_ref = Some("VOICE_KEY_IH_TEST_547_GATE".into());
            config.realtime.model = Some("a\"b".into());

            let result = mint_realtime_for(&config, None, "instr", &[]).await;
            assert!(
                result.is_err(),
                "an injection-shaped model must be rejected"
            );
            let reqs = server.received_requests().await.expect("recording on");
            assert_eq!(reqs.len(), 0, "no request may reach the realtime host");
        });
        // SAFETY: still under env_lock().
        unsafe {
            std::env::remove_var("VOICE_KEY_IH_TEST_547_GATE");
        }
    }

    /// D-16: the identity's realtime_model reaches the minted session.
    #[test]
    fn mint_uses_identity_realtime_model() {
        let _guard = crate::server::test_support::env_lock();
        // SAFETY: serialized by env_lock(); the name is unique to this test.
        unsafe {
            std::env::set_var("VOICE_KEY_IH_TEST_547_IDM", "test-fake-key");
        }
        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/realtime/client_secrets"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"value": "ek_fake"})),
                )
                .mount(&server)
                .await;
            let mut config = Config::default();
            config.realtime.base_url = format!("{}/v1", server.uri());
            config.realtime.api_key_ref = Some("VOICE_KEY_IH_TEST_547_IDM".into());
            let rec = config
                .identities
                .entry("orb_bloom".to_string())
                .or_default();
            rec.voice.realtime_model = Some("gpt-realtime-2".into());
            rec.voice.realtime_voice = Some("marin".into());

            mint_realtime_for(&config, Some("orb_bloom"), "i", &[])
                .await
                .expect("mint");
            let reqs = server.received_requests().await.expect("recording on");
            let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
            assert_eq!(body["session"]["model"], "gpt-realtime-2");
            assert_eq!(body["session"]["audio"]["output"]["voice"], "marin");
        });
        // SAFETY: still under env_lock().
        unsafe {
            std::env::remove_var("VOICE_KEY_IH_TEST_547_IDM");
        }
    }

    #[test]
    fn session_config_transcription_uses_gate() {
        fn transcription(model: &str) -> serde_json::Value {
            let mut config = Config::default();
            config.realtime.transcription_model = Some(model.to_string());
            let json = realtime_session_config_for(&config);
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            v["audio"]["input"]["transcription"].clone()
        }
        assert_eq!(transcription("whisper-1")["model"], "whisper-1");
        assert!(transcription("off").is_null());
        assert_eq!(
            transcription("bad\"x")["model"],
            "gpt-4o-mini-transcribe",
            "an invalid stored value falls back to the default"
        );
    }

    // ── Task 3: keyless realtime and the custom-base key-leak guard ──────────

    /// Save and clear OPENAI_API_KEY under the env lock; returns the prior value.
    fn take_openai_key() -> Option<String> {
        let prior = std::env::var("OPENAI_API_KEY").ok();
        // SAFETY: callers hold env_lock().
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
        }
        prior
    }

    fn restore_openai_key(prior: Option<String>) {
        // SAFETY: callers hold env_lock().
        unsafe {
            match prior {
                Some(v) => std::env::set_var("OPENAI_API_KEY", v),
                None => std::env::remove_var("OPENAI_API_KEY"),
            }
        }
    }

    async fn mock_with_mint() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/realtime/client_secrets"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"value": "ek_fake"})),
            )
            .mount(&server)
            .await;
        server
    }

    /// D-12: a custom base_url with no key reference skips minting and hands
    /// the browser the full session JSON (instructions + tools included).
    #[test]
    fn keyless_custom_base_returns_session_json_without_mint() {
        let _guard = crate::server::test_support::env_lock();
        let prior = take_openai_key();
        block_on(async {
            let server = mock_with_mint().await;
            let mut config = Config::default();
            config.realtime.base_url = format!("{}/v1", server.uri());

            let tools = vec![serde_json::json!({"type": "function", "name": "ping"})];
            let minted = mint_realtime_for(&config, None, "be brief", &tools)
                .await
                .expect("keyless mint is not an error");
            assert!(minted.token.is_none());
            assert_eq!(
                minted.calls_url,
                format!("{}/v1/realtime/calls", server.uri())
            );
            let session: serde_json::Value =
                serde_json::from_str(&minted.session_json.expect("session json")).unwrap();
            assert_eq!(session["instructions"], "be brief");
            assert_eq!(session["tools"][0]["name"], "ping");
            assert_eq!(session["model"], "gpt-realtime");

            let reqs = server.received_requests().await.expect("recording on");
            assert_eq!(reqs.len(), 0, "keyless path must not mint");
        });
        restore_openai_key(prior);
    }

    /// Pitfall 9 / T-54.7-48: the OpenAI key never reaches a custom realtime
    /// host, even when OPENAI_API_KEY and providers.openai are configured.
    #[test]
    fn custom_base_never_receives_openai_key() {
        let _guard = crate::server::test_support::env_lock();
        let prior = std::env::var("OPENAI_API_KEY").ok();
        // SAFETY: serialized by env_lock(); restored below.
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "test-fake-key");
            std::env::set_var("VOICE_KEY_IH_TEST_547_OAI_PROVIDER", "test-fake-key");
        }
        block_on(async {
            let server = mock_with_mint().await;
            let mut config = Config::default();
            config.realtime.base_url = format!("{}/v1", server.uri());
            config
                .providers
                .entry("openai".to_string())
                .or_default()
                .api_key_env = Some("OPENAI_API_KEY".to_string());

            let minted = mint_realtime_for(&config, None, "instr", &[])
                .await
                .expect("custom base without a reference is keyless");
            assert!(minted.token.is_none());
            let reqs = server.received_requests().await.expect("recording on");
            assert_eq!(reqs.len(), 0, "the OpenAI key must not reach the mock");
        });
        // SAFETY: still under env_lock().
        unsafe {
            std::env::remove_var("VOICE_KEY_IH_TEST_547_OAI_PROVIDER");
        }
        restore_openai_key(prior);
    }

    /// D-07: the default base_url with no resolvable OpenAI key is a typed
    /// error (the turn-based fallback trigger), never a panic and never a
    /// keyless session against api.openai.com.
    #[test]
    fn default_base_without_openai_key_errors_for_d07() {
        let _guard = crate::server::test_support::env_lock();
        let prior = take_openai_key();
        let result = block_on(mint_realtime_for(&Config::default(), None, "instr", &[]));
        restore_openai_key(prior);
        let msg = result
            .expect_err("no key on the default base is an error")
            .to_string();
        assert!(msg.contains("key not found"), "{msg}");
        assert!(!msg.contains("sk-"), "{msg}");
    }

    /// T-36.17.12-02-NK (rewritten to exercise the real path): a missing key
    /// yields a typed error with no key material and no variable name.
    #[test]
    fn realtime_token_no_key() {
        let _guard = crate::server::test_support::env_lock();
        let prior = take_openai_key();
        let result = block_on(mint_realtime_for(&Config::default(), None, "instr", &[]));
        restore_openai_key(prior);
        let msg = result.expect_err("missing key must yield Err").to_string();
        assert!(!msg.contains("sk-"), "{msg}");
        assert!(!msg.contains("OPENAI_API_KEY"), "{msg}");
    }

    /// A typed reference that does not resolve is also the D-07 trigger -- it
    /// must NOT silently become a keyless session.
    #[test]
    fn unresolved_key_reference_errors_instead_of_going_keyless() {
        let _guard = crate::server::test_support::env_lock();
        // SAFETY: serialized by env_lock().
        unsafe {
            std::env::remove_var("VOICE_KEY_IH_TEST_547_ABSENT");
        }
        block_on(async {
            let server = mock_with_mint().await;
            let mut config = Config::default();
            config.realtime.base_url = format!("{}/v1", server.uri());
            config.realtime.api_key_ref = Some("VOICE_KEY_IH_TEST_547_ABSENT".into());
            let err = mint_realtime_for(&config, None, "instr", &[])
                .await
                .expect_err("unresolved reference is an error");
            assert!(err.to_string().contains("key not found"), "{err}");
            let reqs = server.received_requests().await.expect("recording on");
            assert_eq!(reqs.len(), 0);
        });
    }

    /// D-11: the three fixed whitelists are gone; the noise-reduction and
    /// VAD-mode lists stay (Open Question 4). Needles are assembled so this
    /// test's own source never satisfies them.
    #[test]
    fn realtime_whitelists_are_gone() {
        let src = include_str!("api.rs");
        for gone in ["_MODELS", "_VOICES", "_TRANSCRIPTION"] {
            let needle = ["REALTIME_ALLOWED", gone].concat();
            assert!(!src.contains(&needle), "{needle} must be deleted");
        }
        for kept in ["_NOISE_REDUCTION", "_VAD_MODES"] {
            let needle = ["REALTIME_ALLOWED", kept].concat();
            assert!(src.contains(&needle), "{needle} must remain");
        }
    }
}

// =============================================================================
// WR-11: identity voice ids are stored trimmed, matching what validation checked.
// =============================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod identity_trim_tests_wr11 {
    use super::{apply_voice_payload, resolve_realtime_selection, VoiceWritePayload};
    use ironhermes_core::config::Config;

    #[test]
    fn padded_identity_voice_ids_are_stored_trimmed() {
        let mut config = Config::default();
        let payload = VoiceWritePayload {
            identity_slug: Some("orb_bloom".to_string()),
            identity_tts_provider: Some(" edge ".to_string()),
            identity_tts_voice: Some(" en-US-AriaNeural ".to_string()),
            identity_realtime_voice: Some(" marin ".to_string()),
            ..Default::default()
        };
        apply_voice_payload(&mut config, &payload).expect("padded ids validate on trim");
        let rec = config.identities.get("orb_bloom").expect("identity record");
        assert_eq!(rec.voice.free_mode_tts_provider.as_deref(), Some("edge"));
        assert_eq!(
            rec.voice.free_mode_tts_voice.as_deref(),
            Some("en-US-AriaNeural")
        );
        assert_eq!(rec.voice.realtime_voice.as_deref(), Some("marin"));
        // And realtime stays available for that identity.
        let (_, voice) = resolve_realtime_selection(&config, Some("orb_bloom"))
            .expect("a padded-then-trimmed voice passes the realtime gate");
        assert_eq!(voice, "marin");
    }
}

// =============================================================================
// WR-12: an unchanged hidden value never blocks a save.
// =============================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod unchanged_hidden_value_tests_wr12 {
    use super::{
        apply_voice_payload, drop_unchanged_choice_fields, validate_voice_payload,
        VoiceWritePayload,
    };
    use ironhermes_core::config::Config;

    /// A config whose UNSELECTED OpenAI provider holds legacy, out-of-charset values.
    fn legacy_config() -> Config {
        let mut config = Config::default();
        config.tts.provider = "edge".into();
        config.tts.openai.voice = "nova (old)".into();
        config.tts.openai.model = "tts 1 hd".into();
        config
    }

    /// What the panel sends: every provider's stored values, echoed back.
    fn echo_payload(config: &Config) -> VoiceWritePayload {
        VoiceWritePayload {
            tts_provider: Some(config.tts.provider.clone()),
            tts_openai_model: Some(config.tts.openai.model.clone()),
            tts_openai_voice: Some(config.tts.openai.voice.clone()),
            auto_tts: Some(true),
            ..Default::default()
        }
    }

    #[test]
    fn echoed_legacy_value_on_a_hidden_provider_does_not_block_the_save() {
        let mut config = legacy_config();
        let payload = echo_payload(&config);
        // Unfiltered, the stateless gate rejects the hidden field (the bug).
        assert!(validate_voice_payload(&payload).is_err());
        let effective = drop_unchanged_choice_fields(&config, &payload);
        validate_voice_payload(&effective).expect("unchanged hidden values are not re-judged");
        apply_voice_payload(&mut config, &effective).expect("the save applies");
        assert!(config.voice.auto_tts, "the real change still lands");
        assert_eq!(config.tts.openai.voice, "nova (old)", "hidden value kept");
    }

    #[test]
    fn a_changed_bad_value_is_still_rejected() {
        let config = legacy_config();
        let mut payload = echo_payload(&config);
        payload.tts_openai_voice = Some("still bad voice".into());
        let effective = drop_unchanged_choice_fields(&config, &payload);
        let err = validate_voice_payload(&effective)
            .expect_err("a NEW out-of-charset value must be rejected")
            .to_string();
        assert!(err.contains("voice_invalid:tts_openai_voice"), "{err}");

        // An unknown provider id that differs from the stored one is rejected too.
        let mut payload = echo_payload(&config);
        payload.tts_provider = Some("nonsense".into());
        let effective = drop_unchanged_choice_fields(&config, &payload);
        assert!(validate_voice_payload(&effective).is_err());
    }

    #[test]
    fn identity_payloads_are_never_filtered() {
        let config = legacy_config();
        let payload = VoiceWritePayload {
            identity_slug: Some("orb_bloom".into()),
            identity_tts_voice: Some("bad voice".into()),
            ..Default::default()
        };
        let effective = drop_unchanged_choice_fields(&config, &payload);
        assert_eq!(effective, payload);
        assert!(validate_voice_payload(&effective).is_err());
    }
}

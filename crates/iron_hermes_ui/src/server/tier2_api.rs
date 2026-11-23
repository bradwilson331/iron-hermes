//! Phase 58.2 Plan 10 (D-13 / D-23 / D-12): Tier 2 long-term memory server fns for the Memory
//! screen's `Tier 2` tab.
//!
//! The tab is read-mostly: search, list, explain, list staged, and three writes: `Forget`
//! (stage), `Undo` (unstage) and the human-run `Purge now` (D-28). Each `#[server]` fn is a thin wrapper that resolves the
//! running manager's Tier 2 handle (the same handle the agent's `memory_*` tools use) and
//! calls a pure `*_logic` fn, which is where validation, the write gate and the DTO mapping
//! live so they are unit-testable against a real service on a temp home.
//!
//! # Security posture
//!
//! - Every `#[server]` fn sits behind `auth::require_auth` (deny-by-default `is_public`, covered
//!   by `auth::deny_by_default_allowlist`); this module adds no auth code of its own.
//! - Stage and unstage pass the same `security.web_config_write_enabled` gate as the Tier 1
//!   memory edits (`check_memory_write_gate`, fresh `Config::load()`), and the service enforces
//!   `memory.forgetting_enabled` itself. The client-side disable of `Forget` is UX only.
//! - Purge (D-28) is the third write: the same write gate, plus the exact typed word `purge` and
//!   the previewed claim ids. It calls `tier2_maintenance::purge_tier2` directly and never goes
//!   through an agent tool or the tool-approval gate.
//! - The web purge needs a logged-in session (`web_ui.auth`), like `/yolo <N>` (Phase 57 D-22):
//!   with auth off any local process (an agent's `terminal`) is indistinguishable from the human,
//!   so it refuses and points at the TTY-gated CLI. `tier2_purge_preview` issues a single-use,
//!   5-minute ticket bound to that session and the exact previewed ids; `tier2_purge_now` must
//!   redeem it (CR-02). The `HttpOnly` cookie is the part an agent cannot read.
//! - Claim ids are validated with `is_valid_claim_id` and queries/limits are bounded here, before
//!   any handle call. The service's own refusal text is relayed unchanged.
//!
//! Declared WITHOUT a cfg gate (like `menu_api`) so the wasm client gets the `#[server]` call
//! stubs; the `*_logic` fns and their server-only imports are gated.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Whether Tier 2 is attached and whether `Forget` can work.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier2Status {
    pub enabled: bool,
    pub forgetting_enabled: bool,
    /// `security.web_config_write_enabled` read fresh per call (D-33): a UX hint only, the server
    /// gate stays authoritative.
    pub writes_enabled: bool,
    /// CR-02 / D-28 amendment: a logged-in web session exists, so the web purge can work. A UX
    /// hint only; `tier2_purge_now_logic` re-checks the session.
    pub purge_login: bool,
}

/// One claim as the list and the staged panel show it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier2ClaimRow {
    pub claim_id: String,
    pub text: String,
    pub confidence: f64,
    pub source: String,
    pub created_at: String,
}

/// Search response: `mode` is `"semantic"` or `"keyword"`; `note` explains a keyword degrade.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier2SearchView {
    pub rows: Vec<Tier2ClaimRow>,
    pub mode: String,
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier2EvidenceRow {
    pub source: String,
    pub quote: String,
    pub timestamp: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier2ExplainView {
    pub claim_id: String,
    pub text: String,
    pub confidence: f64,
    pub evidence: Vec<Tier2EvidenceRow>,
    pub supersedes: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier2ForgetResult {
    pub claim_id: String,
    pub status: String,
    pub note: String,
}

/// D-34: counts only; no per-line daily-log text.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier2PurgePreviewRow {
    pub claim_id: String,
    pub text: String,
    pub predecessors: Vec<String>,
    /// `MEMORY.md: <text>` or `USER.md: <text>`.
    pub tier1_entries: Vec<String>,
    pub live_lines: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier2PurgePreviewView {
    pub rows: Vec<Tier2PurgePreviewRow>,
    pub redacted_quotes: usize,
    pub skipped: Option<String>,
    /// CR-02: the purge ticket for exactly these rows; `None` without a logged-in session.
    pub nonce: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier2PurgeResultView {
    pub purged: usize,
    pub tier1_lines_removed: usize,
    pub live_lines_retracted: usize,
    pub not_staged: Vec<String>,
    pub skipped: Option<String>,
    pub scrub_incomplete: Option<String>,
}

#[cfg(feature = "server")]
pub(crate) type MgrArc =
    std::sync::Arc<tokio::sync::Mutex<ironhermes_agent::memory::MemoryManager>>;

#[cfg(feature = "server")]
pub(crate) type MaybeManager = Option<MgrArc>;

#[cfg(feature = "server")]
pub(crate) type MaybeHandle = Option<std::sync::Arc<dyn ironhermes_core::tier2::Tier2Handle>>;

#[cfg(feature = "server")]
const TIER2_OFF: &str = "Tier 2 memory is off: set memory.tier2_enabled to true in config.yaml.";

/// CR-02: the one outstanding web purge ticket (single operator; a new preview replaces it).
#[cfg(feature = "server")]
pub(crate) struct PurgeTicket {
    nonce: String,
    session: String,
    ids: Vec<String>,
    issued: std::time::Instant,
}

#[cfg(feature = "server")]
pub(crate) type TicketSlot = std::sync::Mutex<Option<PurgeTicket>>;

#[cfg(feature = "server")]
static PURGE_TICKET: TicketSlot = std::sync::Mutex::new(None);

#[cfg(feature = "server")]
const PURGE_TICKET_TTL: std::time::Duration = std::time::Duration::from_secs(300);

#[cfg(feature = "server")]
const PURGE_NEEDS_LOGIN: &str = "Purging from the web needs web login (web_ui.auth). With login off, run `ironhermes memory purge` in a terminal.";

#[cfg(feature = "server")]
const PURGE_TICKET_STALE: &str =
    "This purge preview has expired or was already used. Open Purge now again.";

#[cfg(feature = "server")]
fn sorted(ids: &[String]) -> Vec<String> {
    let mut v = ids.to_vec();
    v.sort_unstable();
    v.dedup();
    v
}

#[cfg(feature = "server")]
fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

#[cfg(feature = "server")]
fn issue_ticket(slot: &TicketSlot, session: &str, ids: &[String]) -> String {
    use base64::Engine as _;
    use rand::RngCore as _;
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    *slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(PurgeTicket {
        nonce: nonce.clone(),
        session: session.to_string(),
        ids: sorted(ids),
        issued: std::time::Instant::now(),
    });
    nonce
}

/// Takes the ticket when nonce, session and id set all match and it is fresh; a mismatch leaves
/// it in place (a 256-bit nonce cannot be guessed, and a stray request must not cancel the
/// human's preview).
#[cfg(feature = "server")]
fn redeem_ticket(
    slot: &TicketSlot,
    session: &str,
    nonce: &str,
    ids: &[String],
) -> Result<PurgeTicket, String> {
    let mut g = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let ok = g.as_ref().is_some_and(|t| {
        t.issued.elapsed() <= PURGE_TICKET_TTL
            && ct_eq(&t.nonce, nonce)
            && ct_eq(&t.session, session)
            && t.ids == sorted(ids)
    });
    if !ok {
        return Err(PURGE_TICKET_STALE.to_string());
    }
    Ok(g.take().expect("checked above"))
}

/// `limit` clamped to 1..=100 (T-58.2-39).
#[cfg(feature = "server")]
fn clamp_limit(limit: u32) -> usize {
    limit.clamp(1, 100) as usize
}

#[cfg(feature = "server")]
fn check_claim_id(claim_id: &str) -> Result<(), String> {
    if ironhermes_core::tier2::is_valid_claim_id(claim_id) {
        Ok(())
    } else {
        Err("invalid claim id".to_string())
    }
}

#[cfg(feature = "server")]
fn to_row(h: ironhermes_core::tier2::Tier2Hit) -> Tier2ClaimRow {
    Tier2ClaimRow {
        claim_id: h.claim_id,
        text: h.text,
        confidence: h.confidence,
        source: h.source,
        created_at: h.created_at,
    }
}

#[cfg(feature = "server")]
pub(crate) fn tier2_status_logic(
    handle: &MaybeHandle,
    config: &ironhermes_core::config::Config,
    logged_in: bool,
) -> Tier2Status {
    Tier2Status {
        enabled: handle.is_some(),
        forgetting_enabled: config.memory.forgetting_enabled,
        writes_enabled: config.security.web_config_write_enabled,
        purge_login: logged_in,
    }
}

/// `query` is trimmed and must be 1-500 characters.
#[cfg(feature = "server")]
pub(crate) async fn tier2_search_logic(
    handle: &MaybeHandle,
    query: &str,
    limit: u32,
) -> Result<Tier2SearchView, String> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > 500 {
        return Err("query must be 1-500 characters".to_string());
    }
    let Some(h) = handle else {
        return Ok(Tier2SearchView {
            rows: Vec::new(),
            mode: "keyword".to_string(),
            note: None,
        });
    };
    let r = h
        .search(query, clamp_limit(limit))
        .await
        .map_err(|e| e.to_string())?;
    let mode = match r.mode {
        ironhermes_core::tier2::Tier2SearchMode::Semantic => "semantic",
        ironhermes_core::tier2::Tier2SearchMode::Keyword => "keyword",
    };
    Ok(Tier2SearchView {
        rows: r.results.into_iter().map(to_row).collect(),
        mode: mode.to_string(),
        note: r.note,
    })
}

#[cfg(feature = "server")]
pub(crate) async fn tier2_list_logic(
    handle: &MaybeHandle,
    limit: u32,
) -> Result<Vec<Tier2ClaimRow>, String> {
    let Some(h) = handle else {
        return Ok(Vec::new());
    };
    let hits = h
        .list_recent(clamp_limit(limit))
        .await
        .map_err(|e| e.to_string())?;
    Ok(hits.into_iter().map(to_row).collect())
}

#[cfg(feature = "server")]
pub(crate) async fn tier2_explain_logic(
    handle: &MaybeHandle,
    claim_id: &str,
) -> Result<Option<Tier2ExplainView>, String> {
    check_claim_id(claim_id)?;
    let Some(h) = handle else {
        return Ok(None);
    };
    let e = h.explain(claim_id).await.map_err(|e| e.to_string())?;
    Ok(e.map(|x| Tier2ExplainView {
        claim_id: x.claim_id,
        text: x.text,
        confidence: x.confidence,
        evidence: x
            .evidence
            .into_iter()
            .map(|v| Tier2EvidenceRow {
                source: v.source,
                quote: v.quote,
                timestamp: v.timestamp,
            })
            .collect(),
        supersedes: x.supersedes,
    }))
}

#[cfg(feature = "server")]
pub(crate) async fn tier2_list_staged_logic(
    handle: &MaybeHandle,
) -> Result<Vec<Tier2ClaimRow>, String> {
    let Some(h) = handle else {
        return Ok(Vec::new());
    };
    let hits = h.list_staged().await.map_err(|e| e.to_string())?;
    Ok(hits.into_iter().map(to_row).collect())
}

/// Shared by stage and unstage: id check, then the Tier 1 edit write gate, then the handle.
/// The service's own refusal text (`memory.forgetting_enabled`, not found) is relayed unchanged.
#[cfg(feature = "server")]
fn write_prelude<'a>(
    handle: &'a MaybeHandle,
    config: &ironhermes_core::config::Config,
    claim_id: &str,
) -> Result<&'a std::sync::Arc<dyn ironhermes_core::tier2::Tier2Handle>, String> {
    check_claim_id(claim_id)?;
    crate::server::api::check_memory_write_gate(config)?;
    handle.as_ref().ok_or_else(|| TIER2_OFF.to_string())
}

#[cfg(feature = "server")]
fn to_result(o: ironhermes_core::tier2::ForgetOutcome) -> Tier2ForgetResult {
    Tier2ForgetResult {
        claim_id: o.claim_id,
        status: o.status,
        note: o.note,
    }
}

#[cfg(feature = "server")]
pub(crate) async fn tier2_stage_forget_logic(
    handle: &MaybeHandle,
    config: &ironhermes_core::config::Config,
    claim_id: &str,
) -> Result<Tier2ForgetResult, String> {
    let h = write_prelude(handle, config, claim_id)?;
    h.stage_forget(claim_id, None)
        .await
        .map(to_result)
        .map_err(|e| e.to_string())
}

#[cfg(feature = "server")]
pub(crate) async fn tier2_unstage_forget_logic(
    handle: &MaybeHandle,
    config: &ironhermes_core::config::Config,
    claim_id: &str,
) -> Result<Tier2ForgetResult, String> {
    let h = write_prelude(handle, config, claim_id)?;
    h.unstage_forget(claim_id)
        .await
        .map(to_result)
        .map_err(|e| e.to_string())
}

/// D-28: the dry run. Read-only; the UI shows it before any delete is possible. D-34: counts only,
/// and the core `scrub_pending` flag is not mapped (the UI never runs a scrub-only purge, D-35).
#[cfg(feature = "server")]
pub(crate) async fn tier2_purge_preview_logic(
    mgr: Option<&MgrArc>,
    session: Option<&str>,
    slot: &TicketSlot,
) -> Result<Tier2PurgePreviewView, String> {
    let Some(mgr) = mgr else {
        return Ok(Tier2PurgePreviewView {
            rows: Vec::new(),
            redacted_quotes: 0,
            skipped: Some(TIER2_OFF.to_string()),
            nonce: None,
        });
    };
    let p = ironhermes_agent::memory::tier2_maintenance::preview_tier2(mgr)
        .await
        .map_err(|e| e.to_string())?;
    let ids: Vec<String> = p.claims.iter().map(|c| c.claim_id.clone()).collect();
    let nonce = match session {
        Some(s) if p.skipped.is_none() && !ids.is_empty() => Some(issue_ticket(slot, s, &ids)),
        _ => None,
    };
    Ok(Tier2PurgePreviewView {
        nonce,
        rows: p
            .claims
            .into_iter()
            .map(|c| Tier2PurgePreviewRow {
                claim_id: c.claim_id,
                text: c.text,
                predecessors: c.predecessors,
                tier1_entries: c
                    .tier1_entries
                    .into_iter()
                    .map(|(target, text)| {
                        let file = if target == "user" {
                            "USER.md"
                        } else {
                            "MEMORY.md"
                        };
                        format!("{file}: {text}")
                    })
                    .collect(),
                live_lines: c.live_lines,
            })
            .collect(),
        redacted_quotes: p.redacted_quotes,
        skipped: p.skipped,
    })
}

/// D-28/D-33: permanently delete the previewed staged claims. Order: write gate, login, typed
/// word, ids, ticket (CR-02), manager. The engine's own `forgetting_enabled` refusal is relayed
/// unchanged.
#[cfg(feature = "server")]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn tier2_purge_now_logic(
    mgr: Option<&MgrArc>,
    config: &ironhermes_core::config::Config,
    session: Option<&str>,
    slot: &TicketSlot,
    nonce: &str,
    confirm: &str,
    claim_ids: &[String],
) -> Result<Tier2PurgeResultView, String> {
    crate::server::api::check_memory_write_gate(config)?;
    let session = session.ok_or_else(|| PURGE_NEEDS_LOGIN.to_string())?;
    if confirm.trim() != "purge" {
        return Err("type purge to confirm".to_string());
    }
    // No upper cap (WR-05): the ids must equal the server's own preview (the ticket), so the
    // preview already bounds them, and a cap made a large staged list unpurgeable.
    if claim_ids.is_empty() {
        return Err("purge needs at least one claim id".to_string());
    }
    for id in claim_ids {
        check_claim_id(id)?;
    }
    let ticket = redeem_ticket(slot, session, nonce, claim_ids)?;
    let mgr = mgr.ok_or_else(|| TIER2_OFF.to_string())?;
    let r = ironhermes_agent::memory::tier2_maintenance::purge_tier2(
        mgr,
        ironhermes_core::tier2::PurgeSurface::Ui,
        claim_ids,
    )
    .await
    .map_err(|e| e.to_string())?;
    // Nothing ran (leases busy, forgetting off): the preview stays usable for a retry.
    if r.skipped.is_some() {
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ticket);
    }
    Ok(Tier2PurgeResultView {
        purged: r.purged.len(),
        tier1_lines_removed: r.tier1_lines_removed,
        live_lines_retracted: r.live_lines_retracted,
        not_staged: r.not_staged,
        skipped: r.skipped,
        scrub_incomplete: r.scrub_incomplete,
    })
}

/// The running manager itself (the purge needs it, not only the Tier 2 handle).
#[cfg(feature = "server")]
fn active_manager() -> MaybeManager {
    crate::server::state::global_app_state()
        .memory_manager
        .clone()
}

/// The running manager's Tier 2 handle; the lock is held only for the clone.
#[cfg(feature = "server")]
async fn active_handle() -> MaybeHandle {
    let state = crate::server::state::global_app_state();
    let mgr = state.memory_manager.as_ref()?;
    let h = mgr.lock().await.tier2();
    h
}

/// CR-02: the caller's session token, only when the auth layer is on (it has then already
/// validated the cookie in `require_auth`).
#[cfg(feature = "server")]
async fn web_session() -> Option<String> {
    if !crate::server::state::web_auth_enabled() {
        return None;
    }
    let headers = dioxus::fullstack::FullstackContext::extract::<axum::http::HeaderMap, _>()
        .await
        .ok()?;
    crate::server::auth::session_token(&headers)
}

#[cfg(feature = "server")]
fn fresh_config() -> Result<ironhermes_core::config::Config, ServerFnError> {
    ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))
}

#[server]
pub async fn tier2_status() -> Result<Tier2Status, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let logged_in = web_session().await.is_some();
        Ok(tier2_status_logic(
            &active_handle().await,
            &fresh_config()?,
            logged_in,
        ))
    }
    #[cfg(not(feature = "server"))]
    {
        Err(ServerFnError::new(
            "tier2_status unavailable without `server` feature",
        ))
    }
}

#[server]
pub async fn tier2_search(query: String, limit: u32) -> Result<Tier2SearchView, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tier2_search_logic(&active_handle().await, &query, limit)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (query, limit);
        Err(ServerFnError::new(
            "tier2_search unavailable without `server` feature",
        ))
    }
}

/// Recent claims. The result has an empty `mode`: the recent list has no search mode.
#[server]
pub async fn tier2_list(limit: u32) -> Result<Tier2SearchView, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let rows = tier2_list_logic(&active_handle().await, limit)
            .await
            .map_err(ServerFnError::new)?;
        Ok(Tier2SearchView {
            rows,
            mode: String::new(),
            note: None,
        })
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = limit;
        Err(ServerFnError::new(
            "tier2_list unavailable without `server` feature",
        ))
    }
}

#[server]
pub async fn tier2_explain(claim_id: String) -> Result<Option<Tier2ExplainView>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tier2_explain_logic(&active_handle().await, &claim_id)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = claim_id;
        Err(ServerFnError::new(
            "tier2_explain unavailable without `server` feature",
        ))
    }
}

#[server]
pub async fn tier2_list_staged() -> Result<Vec<Tier2ClaimRow>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tier2_list_staged_logic(&active_handle().await)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        Err(ServerFnError::new(
            "tier2_list_staged unavailable without `server` feature",
        ))
    }
}

/// D-13/D-21: stage a claim for forgetting (hidden, not deleted; D-27). Same write gate as the Tier 1 edits.
#[server]
pub async fn tier2_stage_forget(claim_id: String) -> Result<Tier2ForgetResult, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tier2_stage_forget_logic(&active_handle().await, &fresh_config()?, &claim_id)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = claim_id;
        Err(ServerFnError::new(
            "tier2_stage_forget unavailable without `server` feature",
        ))
    }
}

/// D-23: cancel a staged forget before the purge. Same write gate as the Tier 1 edits.
#[server]
pub async fn tier2_unstage_forget(claim_id: String) -> Result<Tier2ForgetResult, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tier2_unstage_forget_logic(&active_handle().await, &fresh_config()?, &claim_id)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = claim_id;
        Err(ServerFnError::new(
            "tier2_unstage_forget unavailable without `server` feature",
        ))
    }
}

/// D-28: the dry-run preview of what a purge would delete. Read-only.
#[server]
pub async fn tier2_purge_preview() -> Result<Tier2PurgePreviewView, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let session = web_session().await;
        tier2_purge_preview_logic(active_manager().as_ref(), session.as_deref(), &PURGE_TICKET)
            .await
            .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        Err(ServerFnError::new(
            "tier2_purge_preview unavailable without `server` feature",
        ))
    }
}

/// D-28: permanently delete the previewed staged claims. Write gate + login + typed word +
/// the preview's ticket for exactly these ids (CR-02).
#[server]
pub async fn tier2_purge_now(
    confirm: String,
    nonce: String,
    claim_ids: Vec<String>,
) -> Result<Tier2PurgeResultView, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let session = web_session().await;
        tier2_purge_now_logic(
            active_manager().as_ref(),
            &fresh_config()?,
            session.as_deref(),
            &PURGE_TICKET,
            &nonce,
            &confirm,
            &claim_ids,
        )
        .await
        .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (confirm, nonce, claim_ids);
        Err(ServerFnError::new(
            "tier2_purge_now unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use ironhermes_agent::memory::factory::build_memory_manager_at;
    use ironhermes_core::config::{Config, EmbeddingsConfig, MemoryConfig};
    use ironhermes_core::memory_store::MemoryTarget;

    const FACT: &str = "User prefers concise Telegram replies";

    fn memory_cfg(forgetting: bool) -> MemoryConfig {
        MemoryConfig {
            tier2_enabled: true,
            forgetting_enabled: forgetting,
            embeddings: EmbeddingsConfig {
                base_url: Some("http://127.0.0.1:9".into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn cfg(writes: bool, forgetting: bool) -> Config {
        let mut c = Config::default();
        c.security.web_config_write_enabled = writes;
        c.memory = memory_cfg(forgetting);
        c
    }

    /// A real Tier 2 service on `home` with `FACT` indexed (when `seed`), plus its claim id.
    async fn handle_at(
        home: &std::path::Path,
        forgetting: bool,
        seed: bool,
    ) -> (MaybeHandle, Option<String>) {
        let mgr = build_memory_manager_at(&memory_cfg(forgetting), home)
            .await
            .unwrap()
            .expect("memory enabled");
        if seed {
            mgr.lock()
                .await
                .add(MemoryTarget::Memory, FACT)
                .await
                .expect("seed tier1 entry");
        }
        let h = mgr.lock().await.tier2();
        let id = match (&h, seed) {
            (Some(_), true) => {
                let v = tier2_search_logic(&h, "telegram", 20).await.unwrap();
                v.rows
                    .into_iter()
                    .find(|r| r.text == FACT)
                    .map(|r| r.claim_id)
            }
            _ => None,
        };
        (h, id)
    }

    #[test]
    fn tier2_status_logic_reports_enabled_and_flags() {
        let off = tier2_status_logic(&None, &cfg(true, true), false);
        assert!(!off.enabled);
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (h, _) = rt.block_on(handle_at(dir.path(), true, false));
        let on = tier2_status_logic(&h, &cfg(true, true), true);
        assert_eq!(
            on,
            Tier2Status {
                enabled: true,
                forgetting_enabled: true,
                writes_enabled: true,
                purge_login: true
            }
        );
        let on2 = tier2_status_logic(&h, &cfg(false, true), false);
        assert!(on2.enabled && on2.forgetting_enabled && !on2.writes_enabled && !on2.purge_login);
    }

    #[tokio::test]
    async fn tier2_search_logic_maps_hits() {
        let dir = tempfile::tempdir().unwrap();
        let (h, id) = handle_at(dir.path(), true, true).await;
        let id = id.expect("seeded claim is searchable");
        let v = tier2_search_logic(&h, "  telegram  ", 20).await.unwrap();
        let row = v.rows.iter().find(|r| r.claim_id == id).unwrap();
        assert_eq!(row.text, FACT);
        assert!(row.confidence > 0.0);
        assert!(!row.source.is_empty());
        assert_eq!(v.mode, "keyword");
        assert!(v.note.is_some(), "keyword degrade carries the server note");
        assert!(tier2_search_logic(&h, "   ", 20).await.is_err());
        assert!(tier2_search_logic(&h, &"x".repeat(501), 20).await.is_err());
    }

    #[tokio::test]
    async fn tier2_explain_logic_not_found_and_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let (h, id) = handle_at(dir.path(), true, true).await;
        let found = tier2_explain_logic(&h, &id.unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.text, FACT);
        assert!(!found.evidence.is_empty());
        assert_eq!(
            tier2_explain_logic(&h, "claim:000000000000").await.unwrap(),
            None
        );
        let err = tier2_explain_logic(&h, "not-an-id").await.unwrap_err();
        assert_eq!(err, "invalid claim id");
    }

    #[tokio::test]
    async fn tier2_stage_logic_requires_write_gate() {
        let dir = tempfile::tempdir().unwrap();
        let (h, id) = handle_at(dir.path(), true, true).await;
        let id = id.unwrap();
        let err = tier2_stage_forget_logic(&h, &cfg(false, true), &id)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            "Memory edits are disabled while web config writes are off."
        );
        assert!(tier2_list_staged_logic(&h).await.unwrap().is_empty());
        let err = tier2_unstage_forget_logic(&h, &cfg(false, true), &id)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            "Memory edits are disabled while web config writes are off."
        );
    }

    #[tokio::test]
    async fn tier2_stage_then_unstage_logic_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let (h, id) = handle_at(dir.path(), true, true).await;
        let id = id.unwrap();
        let c = cfg(true, true);
        let staged = tier2_stage_forget_logic(&h, &c, &id).await.unwrap();
        assert_eq!(staged.claim_id, id);
        assert!(
            !tier2_list_logic(&h, 50)
                .await
                .unwrap()
                .iter()
                .any(|r| r.claim_id == id),
            "staged claim leaves the list"
        );
        let staged_rows = tier2_list_staged_logic(&h).await.unwrap();
        assert!(staged_rows.iter().any(|r| r.claim_id == id));
        tier2_unstage_forget_logic(&h, &c, &id).await.unwrap();
        assert!(tier2_list_staged_logic(&h).await.unwrap().is_empty());
        assert!(
            tier2_list_logic(&h, 50)
                .await
                .unwrap()
                .iter()
                .any(|r| r.claim_id == id)
        );
        assert!(tier2_stage_forget_logic(&h, &c, "bad").await.is_err());
    }

    #[tokio::test]
    async fn tier2_logic_without_handle_is_empty_not_error() {
        let h: MaybeHandle = None;
        assert!(tier2_list_logic(&h, 50).await.unwrap().is_empty());
        assert!(tier2_list_staged_logic(&h).await.unwrap().is_empty());
        let s = tier2_search_logic(&h, "cats", 20).await.unwrap();
        assert!(s.rows.is_empty());
        assert_eq!(
            tier2_explain_logic(&h, "claim:000000000000").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn tier2_stage_logic_relays_flag_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let (h, id) = handle_at(dir.path(), false, true).await;
        let id = id.unwrap();
        let err = tier2_stage_forget_logic(&h, &cfg(true, false), &id)
            .await
            .unwrap_err();
        assert!(err.contains("memory.forgetting_enabled"), "{err}");
        assert!(tier2_list_staged_logic(&h).await.unwrap().is_empty());
        assert!(
            tier2_list_logic(&h, 50)
                .await
                .unwrap()
                .iter()
                .any(|r| r.claim_id == id)
        );
    }

    #[tokio::test]
    async fn tier2_unstage_logic_relays_flag_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let (h1, id) = handle_at(dir.path(), true, true).await;
        let id = id.unwrap();
        tier2_stage_forget_logic(&h1, &cfg(true, true), &id)
            .await
            .unwrap();
        drop(h1);
        // Second manager on the SAME home with forgetting off (plan 08's flag rule).
        let (h2, _) = handle_at(dir.path(), false, false).await;
        let err = tier2_unstage_forget_logic(&h2, &cfg(true, false), &id)
            .await
            .unwrap_err();
        assert!(err.contains("memory.forgetting_enabled"), "{err}");
        let staged = tier2_list_staged_logic(&h2).await.unwrap();
        assert!(staged.iter().any(|r| r.claim_id == id));
    }

    async fn mgr_at(home: &std::path::Path) -> MgrArc {
        build_memory_manager_at(&memory_cfg(true), home)
            .await
            .unwrap()
            .expect("memory enabled")
    }

    /// Adds `text` to MEMORY.md, stages its claim and returns the claim id.
    async fn staged_claim(mgr: &MgrArc, text: &str) -> String {
        mgr.lock()
            .await
            .add(MemoryTarget::Memory, text)
            .await
            .unwrap();
        let h = mgr.lock().await.tier2();
        let v = tier2_search_logic(&h, "Lisbon", 20).await.unwrap();
        let id = v
            .rows
            .into_iter()
            .find(|r| r.text == text)
            .unwrap()
            .claim_id;
        tier2_stage_forget_logic(&h, &cfg(true, true), &id)
            .await
            .unwrap();
        id
    }

    fn memory_md(home: &std::path::Path) -> String {
        std::fs::read_to_string(home.join("memories/MEMORY.md")).unwrap_or_default()
    }

    const SESSION: &str = "session-a";

    fn slot() -> TicketSlot {
        std::sync::Mutex::new(None)
    }

    /// Preview as the logged-in `SESSION`; returns the ticket nonce.
    async fn preview_nonce(mgr: &MgrArc, slot: &TicketSlot) -> String {
        tier2_purge_preview_logic(Some(mgr), Some(SESSION), slot)
            .await
            .unwrap()
            .nonce
            .expect("a logged-in preview with rows issues a ticket")
    }

    #[tokio::test]
    async fn tier2_purge_roundtrip_removes_tier1_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = mgr_at(dir.path()).await;
        let id = staged_claim(&mgr, "User lives in Lisbon").await;
        assert!(memory_md(dir.path()).contains("Lisbon"));
        let t = slot();
        let p = tier2_purge_preview_logic(Some(&mgr), Some(SESSION), &t)
            .await
            .unwrap();
        let row = p.rows.iter().find(|r| r.claim_id == id).expect("previewed");
        assert!(
            row.tier1_entries
                .contains(&"MEMORY.md: User lives in Lisbon".to_string()),
            "{row:?}"
        );
        let nonce = p.nonce.unwrap();
        let ids = [id];
        let c = cfg(true, true);
        let r = tier2_purge_now_logic(Some(&mgr), &c, Some(SESSION), &t, &nonce, "purge", &ids)
            .await
            .unwrap();
        assert_eq!(r.purged, 1, "{r:?}");
        assert!(!memory_md(dir.path()).contains("Lisbon"));
        // Single use: the same ticket cannot run a second purge.
        let err = tier2_purge_now_logic(Some(&mgr), &c, Some(SESSION), &t, &nonce, "purge", &ids)
            .await
            .unwrap_err();
        assert_eq!(err, PURGE_TICKET_STALE);
    }

    #[tokio::test]
    async fn tier2_purge_now_requires_write_gate() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = mgr_at(dir.path()).await;
        let id = staged_claim(&mgr, "User lives in Lisbon").await;
        let t = slot();
        let nonce = preview_nonce(&mgr, &t).await;
        let err = tier2_purge_now_logic(
            Some(&mgr),
            &cfg(false, true),
            Some(SESSION),
            &t,
            &nonce,
            "purge",
            &[id],
        )
        .await
        .unwrap_err();
        assert_eq!(
            err,
            "Memory edits are disabled while web config writes are off."
        );
        assert!(memory_md(dir.path()).contains("Lisbon"));
    }

    /// CR-02: with web login off, a local process (an agent's `terminal`) could replay the whole
    /// preview-then-confirm flow, so the web purge refuses and issues no ticket.
    #[tokio::test]
    async fn tier2_purge_now_requires_login() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = mgr_at(dir.path()).await;
        let id = staged_claim(&mgr, "User lives in Lisbon").await;
        let t = slot();
        let p = tier2_purge_preview_logic(Some(&mgr), None, &t)
            .await
            .unwrap();
        assert_eq!(p.rows.len(), 1, "the preview itself stays readable");
        assert!(p.nonce.is_none() && t.lock().unwrap().is_none());
        let err = tier2_purge_now_logic(Some(&mgr), &cfg(true, true), None, &t, "", "purge", &[id])
            .await
            .unwrap_err();
        assert_eq!(err, PURGE_NEEDS_LOGIN);
        assert!(memory_md(dir.path()).contains("Lisbon"));
    }

    /// CR-02: the typed word is public, so the purge also needs the preview's ticket, bound to
    /// the session and the exact previewed ids, and fresh.
    #[tokio::test]
    async fn tier2_purge_now_requires_matching_ticket() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = mgr_at(dir.path()).await;
        let id = staged_claim(&mgr, "User lives in Lisbon").await;
        let c = cfg(true, true);
        let t = slot();
        let ids = [id.clone()];
        // No preview at all.
        let err = tier2_purge_now_logic(Some(&mgr), &c, Some(SESSION), &t, "x", "purge", &ids)
            .await
            .unwrap_err();
        assert_eq!(err, PURGE_TICKET_STALE);
        let nonce = preview_nonce(&mgr, &t).await;
        let other = ["claim:000000000000".to_string()];
        let both = [id.clone(), other[0].clone()];
        for (session, n, ids) in [
            (SESSION, "wrong-nonce", &ids[..]),
            ("session-b", nonce.as_str(), &ids[..]),
            (SESSION, nonce.as_str(), &other[..]),
            (SESSION, nonce.as_str(), &both[..]),
        ] {
            let err = tier2_purge_now_logic(Some(&mgr), &c, Some(session), &t, n, "purge", ids)
                .await
                .unwrap_err();
            assert_eq!(err, PURGE_TICKET_STALE, "{session} {n} {ids:?}");
        }
        assert!(memory_md(dir.path()).contains("Lisbon"));
        // Expired.
        t.lock().unwrap().as_mut().unwrap().issued = std::time::Instant::now()
            .checked_sub(PURGE_TICKET_TTL + std::time::Duration::from_secs(1))
            .unwrap();
        let err = tier2_purge_now_logic(Some(&mgr), &c, Some(SESSION), &t, &nonce, "purge", &ids)
            .await
            .unwrap_err();
        assert_eq!(err, PURGE_TICKET_STALE);
        // A fresh preview still works after all those refusals.
        let nonce = preview_nonce(&mgr, &t).await;
        let r = tier2_purge_now_logic(Some(&mgr), &c, Some(SESSION), &t, &nonce, "purge", &ids)
            .await
            .unwrap();
        assert_eq!(r.purged, 1);
    }

    #[tokio::test]
    async fn tier2_purge_now_requires_typed_confirm() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = mgr_at(dir.path()).await;
        let id = staged_claim(&mgr, "User lives in Lisbon").await;
        let c = cfg(true, true);
        let t = slot();
        let nonce = preview_nonce(&mgr, &t).await;
        for bad in ["Purge", "", "purge it"] {
            let err = tier2_purge_now_logic(
                Some(&mgr),
                &c,
                Some(SESSION),
                &t,
                &nonce,
                bad,
                std::slice::from_ref(&id),
            )
            .await
            .unwrap_err();
            assert_eq!(err, "type purge to confirm");
        }
        assert!(memory_md(dir.path()).contains("Lisbon"));
        let r = tier2_purge_now_logic(Some(&mgr), &c, Some(SESSION), &t, &nonce, " purge ", &[id])
            .await
            .unwrap();
        assert_eq!(r.purged, 1);
    }

    #[tokio::test]
    async fn tier2_purge_now_rejects_bad_ids() {
        let c = cfg(true, true);
        let t = slot();
        let s = Some(SESSION);
        // No manager: reaching the id errors (not the Tier-2-off one) proves the checks run first.
        let err = tier2_purge_now_logic(None, &c, s, &t, "n", "purge", &[])
            .await
            .unwrap_err();
        assert!(err.contains("at least one"), "{err}");
        let bad = vec!["not-an-id".to_string()];
        assert_eq!(
            tier2_purge_now_logic(None, &c, s, &t, "n", "purge", &bad)
                .await
                .unwrap_err(),
            "invalid claim id"
        );
        // WR-05: more than 200 previewed ids reach the ticket check, not a size refusal.
        let many: Vec<String> = (0..201).map(|n| format!("claim:{n:012x}")).collect();
        let err = tier2_purge_now_logic(None, &c, s, &t, "n", "purge", &many)
            .await
            .unwrap_err();
        assert_eq!(err, PURGE_TICKET_STALE);
        issue_ticket(&t, SESSION, &many);
        let nonce = t.lock().unwrap().as_ref().unwrap().nonce.clone();
        let err = tier2_purge_now_logic(None, &c, s, &t, &nonce, "purge", &many)
            .await
            .unwrap_err();
        assert_eq!(err, TIER2_OFF, "201 previewed ids pass every check");
    }

    #[tokio::test]
    async fn tier2_purge_logic_without_tier2_is_skipped() {
        let t = slot();
        let p = tier2_purge_preview_logic(None, Some(SESSION), &t)
            .await
            .unwrap();
        assert!(p.rows.is_empty() && p.skipped.is_some() && p.nonce.is_none());
        let ids = vec!["claim:000000000000".to_string()];
        issue_ticket(&t, SESSION, &ids);
        let nonce = t.lock().unwrap().as_ref().unwrap().nonce.clone();
        let err = tier2_purge_now_logic(
            None,
            &cfg(true, true),
            Some(SESSION),
            &t,
            &nonce,
            "purge",
            &ids,
        )
        .await
        .unwrap_err();
        assert_eq!(err, TIER2_OFF);
    }
}

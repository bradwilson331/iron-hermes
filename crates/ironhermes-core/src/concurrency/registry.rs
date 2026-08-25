//! Phase 39.1 (R39.1-05 / D-05 / R39.1-09): Global `TurnRegistry` and associated types.
//!
//! The `TurnRegistry` is the process-wide single source of truth for all in-flight
//! agent turns across every surface (Web, Gateway, CLI, Realtime). It is shared via
//! `Arc<TurnRegistry>` — callers clone the Arc; the inner `RwLock` serialises all
//! register/deregister operations.
//!
//! # Register-before-spawn discipline
//!
//! **Callers MUST call `register` before spawning the tokio task** that will run the
//! agent turn. This ensures the registry entry is visible to all observers before any
//! output is produced. Deregister in the task's finally/drop path (on completion,
//! panic, or cancellation).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;

/// Unique identifier for a single in-flight agent turn (UUID v4 per R39.1-09 security
/// note: opaque, unguessable identifiers resist turn-id enumeration attacks at the
/// `/agents cancel <id>` boundary — T-39.1-01).
pub type TurnId = uuid::Uuid;

/// The surface (platform) that originated a turn.
///
/// - `Web`       — Web UI WebSocket session (iron_hermes_ui).
/// - `Gateway`   — Telegram/Discord/Slack gateway session (ironhermes-gateway).
/// - `Cli`       — CLI / TUI session (ironhermes-cli, tui_rata).
/// - `Realtime`  — Web voice2voice session registered at ephemeral-token mint time
///   (iron_hermes_ui realtime_session.rs, Plan 07). WebRTC-direct turns are registered
///   here so the global registry shows voice turns alongside text turns.
/// - `ApiServer` — REST API server session (ironhermes-restgw, Phase 36.7.1 Plan 06).
///   A turn submitted through `POST /v1/runs` is registered under this surface so a
///   cross-surface listing can distinguish it from a Telegram-, web-, CLI- or
///   realtime-originated turn — the distinction the approval route (T-36.7.1-48) and
///   any future operator view depend on. Any `match` on `Surface` **must cover all
///   five arms**.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Web,
    Gateway,
    Cli,
    Realtime,
    ApiServer,
}

impl std::fmt::Display for Surface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Surface::Web => write!(f, "web"),
            Surface::Gateway => write!(f, "gateway"),
            Surface::Cli => write!(f, "cli"),
            Surface::Realtime => write!(f, "realtime"),
            Surface::ApiServer => write!(f, "api_server"),
        }
    }
}

/// A live in-flight turn entry stored in the registry.
///
/// Contains the `CancellationToken` so callers can cancel the turn directly.
/// Use `TurnSummary` for wire-safe projections (no token).
pub struct TurnEntry {
    pub turn_id: TurnId,
    pub session_id: String,
    pub surface: Surface,
    pub started_at: Instant,
    /// Cancellation handle for this turn. Dropping all holders of this token does NOT
    /// cancel it — callers must explicitly call `cancel()` to signal cancellation.
    /// The token is stored here so `cancel_one` / `cancel_session` can reach it.
    pub cancel: tokio_util::sync::CancellationToken,
}

/// Wire-safe projection of a `TurnEntry` — does not carry the `CancellationToken`.
///
/// Used for serialisation, display, and passing across API boundaries where
/// the raw cancellation handle must not be exposed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnSummary {
    pub turn_id: TurnId,
    pub session_id: String,
    pub surface: Surface,
    /// Wall-clock elapsed milliseconds since the turn was registered.
    pub elapsed_ms: u128,
}

impl TurnSummary {
    /// Construct from a live `TurnEntry`, computing elapsed_ms at call time.
    pub fn from_entry(entry: &TurnEntry) -> Self {
        Self {
            turn_id: entry.turn_id,
            session_id: entry.session_id.clone(),
            surface: entry.surface,
            elapsed_ms: entry.started_at.elapsed().as_millis(),
        }
    }
}

/// Process-wide registry of all in-flight agent turns across every surface.
///
/// Cloning the registry returns a second handle to the **same** inner map — the
/// `Arc<RwLock<…>>` ensures a single source of truth.
///
/// # Usage
///
/// ```ignore
/// let registry = TurnRegistry::new();
/// let entry = TurnEntry {
///     turn_id: TurnId::new_v4(),
///     session_id: "s1".to_string(),
///     surface: Surface::Web,
///     started_at: std::time::Instant::now(),
///     cancel: tokio_util::sync::CancellationToken::new(),
/// };
/// registry.register(entry).await;
/// // … spawn agent task …
/// registry.deregister(turn_id).await;
/// ```
#[derive(Clone)]
pub struct TurnRegistry(Arc<RwLock<HashMap<TurnId, TurnEntry>>>);

impl Default for TurnRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnRegistry {
    /// Create a new, empty registry.
    pub fn new() -> Self {
        TurnRegistry(Arc::new(RwLock::new(HashMap::new())))
    }

    /// Register a new in-flight turn.
    ///
    /// **Must be called BEFORE spawning the task** — this ensures the entry is
    /// observable to all concurrent readers immediately (register-before-spawn
    /// discipline).
    pub async fn register(&self, entry: TurnEntry) {
        let mut map = self.0.write().await;
        map.insert(entry.turn_id, entry);
    }

    /// Remove a completed (or cancelled) turn from the registry.
    pub async fn deregister(&self, id: TurnId) {
        let mut map = self.0.write().await;
        map.remove(&id);
    }

    /// Cancel one specific turn by id.
    ///
    /// Returns `true` if the entry was found and `cancel()` was called on its token.
    /// Returns `false` if no entry exists for `id` (race: already deregistered).
    ///
    /// Note: calling `cancel()` signals the token but does NOT remove the entry from
    /// the registry. The task is responsible for calling `deregister` on completion.
    pub async fn cancel_one(&self, id: TurnId) -> bool {
        let map = self.0.read().await;
        if let Some(entry) = map.get(&id) {
            entry.cancel.cancel();
            true
        } else {
            false
        }
    }

    /// Cancel all in-flight turns for a session.
    ///
    /// Returns the number of turns whose tokens were cancelled.
    ///
    /// Note: entries remain in the registry until each task calls `deregister`.
    pub async fn cancel_session(&self, session_id: &str) -> usize {
        let map = self.0.read().await;
        let mut count = 0usize;
        for entry in map.values() {
            if entry.session_id == session_id {
                entry.cancel.cancel();
                count += 1;
            }
        }
        count
    }

    /// Look up a single in-flight turn by identifier, projected through
    /// [`TurnSummary::from_entry`] so the cancellation token never crosses this
    /// boundary — mirrors the read-lock discipline of [`Self::list_all`].
    ///
    /// Returns `None` for an unknown OR already-deregistered identifier rather than a
    /// default-valued summary — callers (e.g. the REST `/v1/runs/{id}` status route)
    /// need to distinguish "this run never existed / already finished" from a live
    /// entry, and a fabricated placeholder summary would erase that distinction.
    pub async fn get_one(&self, id: TurnId) -> Option<TurnSummary> {
        let map = self.0.read().await;
        map.get(&id).map(TurnSummary::from_entry)
    }

    /// List all in-flight turns as wire-safe summaries.
    pub async fn list_all(&self) -> Vec<TurnSummary> {
        let map = self.0.read().await;
        map.values().map(TurnSummary::from_entry).collect()
    }

    /// List in-flight turns for a specific session.
    pub async fn list_session(&self, session_id: &str) -> Vec<TurnSummary> {
        let map = self.0.read().await;
        map.values()
            .filter(|e| e.session_id == session_id)
            .map(TurnSummary::from_entry)
            .collect()
    }

    /// Return a clone of the `CancellationToken` for a specific turn, if it exists.
    ///
    /// Used by `realtime_heartbeat` (Plan 07) to check `is_cancelled()` without
    /// calling `cancel()`. Returns `None` if the turn has already been deregistered
    /// (race — treat as Cancelled at the call site).
    pub async fn get_cancel_token(
        &self,
        id: TurnId,
    ) -> Option<tokio_util::sync::CancellationToken> {
        let map = self.0.read().await;
        map.get(&id).map(|e| e.cancel.clone())
    }

    /// Count in-flight turns for a specific session.
    ///
    /// Used by the `/new`/`/reset` warn helper (`in_flight_warning`) to decide
    /// whether to emit a "turns still running" advisory (R39.1-07).
    pub async fn count_session(&self, session_id: &str) -> usize {
        let map = self.0.read().await;
        map.values().filter(|e| e.session_id == session_id).count()
    }
}

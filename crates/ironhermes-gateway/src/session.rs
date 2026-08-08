use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use ironhermes_core::ChatMessage;
use ironhermes_core::types::Role;
use ironhermes_state::StateStore;
use std::collections::HashMap;
use tracing::{debug, warn};

// Phase 36.17.3 (Resolution 3 / D-01): `SessionKey` was relocated into
// `ironhermes-core::session` so every `MessageQueue<K>` consumer (gateway,
// TUI, future web/Discord/Slack/REST) shares the same canonical key. The
// re-export below preserves backward compatibility for the gateway-internal
// `use crate::session::SessionKey;` call sites in `session_queue.rs` and
// `user_queue.rs`, and for any downstream `use ironhermes_gateway::session::SessionKey;`.
pub use ironhermes_core::session::SessionKey;

/// Phase 39.1 (R39.1-02, D-02): Atomically append a (user_msg, assistant_msg) pair to a
/// shared history handle.  Both messages are pushed under ONE short lock acquisition so no
/// concurrent turn can interleave its pair between the two pushes.  The lock is NEVER held
/// across an `.await` — callers must only call this at turn completion (after all streaming
/// has finished).
pub fn append_turn_pair(
    history: &Arc<Mutex<Vec<ChatMessage>>>,
    user_msg: ChatMessage,
    assistant_msg: ChatMessage,
) {
    let mut guard = history.lock().unwrap_or_else(|e| e.into_inner());
    guard.push(user_msg);
    guard.push(assistant_msg);
    // guard dropped here — lock released before any await
}

/// An active gateway conversation session.
///
/// Phase 36: `running` flag drives per-session agent guard (D-03/D-05/D-06).
#[derive(Debug, Clone)]
pub struct GatewaySession {
    pub key: SessionKey,
    pub session_id: String,
    pub messages: Vec<ChatMessage>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: String,
    /// Per-session running flag (D-03/D-05, Phase 36).
    /// `RunningAgentGuard` sets this true on construction and false on Drop (D-06).
    /// Never set/cleared directly — RAII-only contract prevents "forgot a branch" bugs.
    pub running: Arc<AtomicBool>,
    /// Phase 36.17.9: persisted voice mode for this conversation — `off` | `on` |
    /// `tts`. Loaded from `gateway_routes` on resume; `/voice` updates it
    /// write-through via [`SessionStore::set_voice_mode`].
    pub voice_mode: String,
    /// Phase 39.1 (R39.1-02, D-02): Arc-refcounted concurrent history handle.
    ///
    /// Each spawned turn holds a clone of this `Arc` so that when `/new` removes
    /// the `GatewaySession` from `SessionStore`, the turn's clone keeps the Vec
    /// alive and the turn can still append its result messages at completion —
    /// no dangling reference (RESEARCH Pitfall 3).
    ///
    /// Append via [`append_turn_pair`] (one lock acquisition for the pair) or
    /// via [`SessionStore::add_turn_pair_to_session`] which additionally writes
    /// through to SQLite.
    ///
    /// `messages` (plain `Vec`) is kept in sync by `add_message` /
    /// `add_turn_pair_to_session` calls that update BOTH the `Arc`-guarded Vec
    /// AND `self.messages` so that readers that clone `s.messages` directly
    /// (e.g. to build the turn's initial context snapshot) still see consistent data.
    pub history: Arc<Mutex<Vec<ChatMessage>>>,
}

impl GatewaySession {
    pub fn new(key: SessionKey, model: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            key,
            messages: Vec::new(),
            history: Arc::new(Mutex::new(Vec::new())),
            created_at: now,
            updated_at: now,
            model: model.into(),
            running: Arc::new(AtomicBool::new(false)),
            voice_mode: "off".to_string(),
        }
    }

    /// Phase 36.17.9: rebuild a session from persisted state on gateway restart.
    /// Carries the existing `session_id` (so new messages append to the same
    /// durable thread), the rehydrated message history, and the saved voice mode.
    pub fn from_persisted(
        key: SessionKey,
        session_id: String,
        messages: Vec<ChatMessage>,
        model: impl Into<String>,
        voice_mode: String,
    ) -> Self {
        let now = Utc::now();
        // The `history` Arc is seeded with the rehydrated message history so that
        // concurrent turns started immediately after resume see the correct starting
        // context when they take their snapshot via `history_arc()`.
        let history = Arc::new(Mutex::new(messages.clone()));
        Self {
            session_id,
            key,
            messages,
            history,
            created_at: now,
            updated_at: now,
            model: model.into(),
            running: Arc::new(AtomicBool::new(false)),
            voice_mode,
        }
    }

    pub fn add_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg.clone());
        if let Ok(mut h) = self.history.lock() {
            h.push(msg);
        }
        self.updated_at = Utc::now();
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        if let Ok(mut h) = self.history.lock() {
            h.clear();
        }
        self.updated_at = Utc::now();
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Returns true if this session has been inactive longer than `timeout_hours`.
    pub fn is_expired(&self, timeout_hours: u64) -> bool {
        let cutoff = Utc::now() - chrono::Duration::hours(timeout_hours as i64);
        self.updated_at < cutoff
    }

    /// Phase 39.1 (R39.1-02): return a clone of the shared history `Arc`.
    ///
    /// Spawned turns call this at turn start to obtain a handle that stays valid
    /// even if `/new` removes this `GatewaySession` from `SessionStore` — the
    /// `Arc` refcount keeps the `Vec` alive for the turn's lifetime (RESEARCH
    /// Pitfall 3).  The turn uses [`append_turn_pair`] on the cloned `Arc` to
    /// append its result messages atomically at completion.
    pub fn history_arc(&self) -> Arc<Mutex<Vec<ChatMessage>>> {
        self.history.clone()
    }
}

/// Write-through session cache: SQLite persistence via StateStore + in-memory HashMap for fast access.
/// Per D-01: every create/add_message writes to SQLite immediately.
/// Per D-02: on restart, fresh session starts — old data is query-only via StateStore.
pub struct SessionStore {
    state: Arc<Mutex<StateStore>>,
    sessions: HashMap<String, GatewaySession>,
    /// Phase 25.3-14 verifier-blocker close-out: per-cwd workspace resolved by
    /// GatewayRunner at startup. Threaded into state.create_session as
    /// workspace_root so /sessions --workspace + Phase 25.4 Curator see
    /// gateway-originated sessions. Set via SessionStore::set_workspace from
    /// GatewayRunner::set_workspace.
    workspace: Option<Arc<ironhermes_core::workspace::Workspace>>,
    /// Phase 25.3-15 CR-02 close-out: per-session trajectory directory root.
    /// Per-session writers nest under
    /// `<trajectory_root>/<canonical_session_id>/trajectories.jsonl`. Replaces
    /// the old process-wide `gateway-<uuid>` writer that was unreachable from
    /// `hermes session export` and decoupled trajectory paths from per-message
    /// canonical session UUIDs.
    trajectory_root: Option<std::path::PathBuf>,
    /// Phase 25.3-15 CR-02 close-out: cached per-session TrajectoryWriter
    /// handles, keyed by the canonical SQLite session UUID. Lazily opened on
    /// first tool call; reused across messages on the same chat to avoid
    /// reopening the file per call (and the resulting file-handle leak).
    trajectory_writers:
        HashMap<String, Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>>,
    /// Phase 36.17.9 (`gateway.persist_sessions`, default true): when true,
    /// `get_or_create` resumes a chat's prior session across restarts via the
    /// durable `gateway_routes` table and persists voice mode. When false, the
    /// old D-02 stateless behavior is restored (fresh session every restart).
    persist_sessions: bool,
}

impl SessionStore {
    /// Phase 47.5 (W4): resume-side bound on history rehydration — a resumed
    /// session starts from the most recent N messages so the context engine
    /// never inherits an unbounded window. The DB keeps everything;
    /// export/Curator read the DB directly and are unaffected.
    pub const MAX_RESUME_REHYDRATE_MESSAGES: usize = 200;

    pub fn new(state: Arc<Mutex<StateStore>>) -> Self {
        Self {
            state,
            sessions: HashMap::new(),
            workspace: None,
            trajectory_root: None,
            trajectory_writers: HashMap::new(),
            persist_sessions: true,
        }
    }

    /// Phase 36.17.9: toggle durable session-routing persistence. Set by
    /// `GatewayRunner` from `config.gateway.persist_sessions` at startup.
    /// `false` restores the legacy stateless behavior (D-02).
    pub fn set_persist_sessions(&mut self, persist: bool) {
        self.persist_sessions = persist;
    }

    /// Phase 25.3-14: install the resolved Workspace so get_or_create can persist
    /// workspace_root onto the sessions table. Mirrors GatewayRunner::set_workspace
    /// — GatewayRunner calls this on the inner SessionStore so both sides agree.
    pub fn set_workspace(&mut self, workspace: Arc<ironhermes_core::workspace::Workspace>) {
        self.workspace = Some(workspace);
    }

    /// Phase 25.3-15 CR-02: install the trajectory directory ROOT under which
    /// per-session subdirs are nested. Set by `GatewayRunner::set_trajectory_root`
    /// during gateway startup. After this is set, `get_or_create_trajectory_writer`
    /// will lazily open per-session writers at
    /// `<trajectory_root>/<session_id>/trajectories.jsonl`.
    pub fn set_trajectory_root(&mut self, root: std::path::PathBuf) {
        self.trajectory_root = Some(root);
    }

    /// Phase 25.3-15 CR-02: get or lazily-open a per-session TrajectoryWriter
    /// handle keyed by the canonical SQLite session UUID. Returns None if no
    /// trajectory_root was installed (gateway launched without trajectory dir
    /// configured) or if the open failed (logged + None — best-effort).
    ///
    /// The returned handle is cached in `trajectory_writers` so subsequent
    /// messages on the same chat reuse the same writer (avoids reopening
    /// the file and a file-handle leak across long-running gateway sessions).
    pub fn get_or_create_trajectory_writer(
        &mut self,
        session_id: &str,
    ) -> Option<Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>> {
        if let Some(existing) = self.trajectory_writers.get(session_id) {
            return Some(existing.clone());
        }
        let root = self.trajectory_root.as_ref()?;
        let traj_path = root.join(session_id).join("trajectories.jsonl");
        match ironhermes_trajectory::TrajectoryWriter::open(&traj_path) {
            Ok(w) => {
                let arc_writer = Arc::new(Mutex::new(w));
                let handle: Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle> =
                    Arc::new(ironhermes_trajectory::TrajectoryWriterHandleImpl::new(
                        arc_writer,
                    ));
                self.trajectory_writers
                    .insert(session_id.to_string(), handle.clone());
                Some(handle)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %traj_path.display(),
                    "Phase 25.3-15: failed to open per-session trajectory writer for {session_id}"
                );
                None
            }
        }
    }

    /// Get or create a session for the given key. On creation, writes through to SQLite.
    ///
    /// Phase 36.17.9: when `persist_sessions` is true and this chat has a durable
    /// route to a still-open prior session, that session is RESUMED (same
    /// `session_id`, message history rehydrated, voice mode restored) instead of
    /// minting a fresh one — so a gateway restart continues the conversation.
    pub fn get_or_create(
        &mut self,
        key: SessionKey,
        model: &str,
        source: &str,
    ) -> &mut GatewaySession {
        let string_key = key.to_string_key();
        if !self.sessions.contains_key(&string_key) {
            let session = self.build_or_resume_session(&string_key, &key, model, source);
            self.sessions.insert(string_key.clone(), session);
        }
        self.sessions.get_mut(&string_key).unwrap()
    }

    /// Build a fresh session OR resume the persisted one for this key (Phase 36.17.9).
    ///
    /// Resume path (persist on + route exists + target session still open):
    /// rehydrate `session_id` + message history + voice mode, and refresh the
    /// route's `updated_at`. Otherwise create a fresh session, persist it
    /// (write-through), and record/refresh its route. On a poisoned state lock
    /// we degrade to an unpersisted in-memory session rather than panicking.
    fn build_or_resume_session(
        &self,
        string_key: &str,
        key: &SessionKey,
        model: &str,
        source: &str,
    ) -> GatewaySession {
        // Phase 25.3-14/16: resolved workspace root (canonical_root_string) threaded
        // into create_session so Telegram rows carry workspace metadata (D-W-1) and
        // /sessions --workspace + the Phase 25.4 Curator see gateway sessions.
        let workspace_root = self.workspace.as_ref().map(|ws| ws.canonical_root_string());

        let Ok(mut state) = self.state.lock() else {
            warn!("state lock poisoned — using unpersisted in-memory session for {string_key}");
            return GatewaySession::new(key.clone(), model);
        };

        // Resume a persisted, still-open session for this key.
        if self.persist_sessions
            && let Ok(Some(route)) = state.get_route(string_key)
            && !route.session_id.is_empty()
            && let Ok(Some(row)) = state.get_session(&route.session_id)
            && row.ended_at.is_none()
        {
            let mut messages = state
                .get_chat_messages(&route.session_id)
                .unwrap_or_default();

            // Phase 47.5 (W4): bound + head-align rehydration on the truncation
            // path only. An under-cap session resumes byte-identically, head
            // untouched (branch 1). Over the cap, drain the front to the most
            // recent MAX_RESUME_REHYDRATE_MESSAGES, then advance the head to the
            // first Role::User so no orphaned tool_result/assistant message leads
            // the rehydrated history (branch 2). If the retained window has no
            // Role::User at all, retain nothing rather than hand a provider a
            // protocol-invalid orphaned head — the DB still has everything, so
            // export/Curator are unaffected (branch 3).
            let total_before = messages.len();
            if total_before > Self::MAX_RESUME_REHYDRATE_MESSAGES {
                let drop_front = total_before - Self::MAX_RESUME_REHYDRATE_MESSAGES;
                messages.drain(0..drop_front);
                match messages.iter().position(|m| m.role == Role::User) {
                    Some(head_idx) => {
                        if head_idx > 0 {
                            messages.drain(0..head_idx);
                        }
                        debug!(
                            "resume rehydration for session {}: bounded+head-aligned, {} of {} message(s) dropped ({} retained)",
                            route.session_id,
                            total_before - messages.len(),
                            total_before,
                            messages.len()
                        );
                    }
                    None => {
                        let retained_before_clear = messages.len();
                        messages.clear();
                        warn!(
                            "resume rehydration for session {}: retained window of {} message(s) contained no Role::User head — clearing to avoid a protocol-invalid orphaned head ({} of {} total message(s) dropped)",
                            route.session_id,
                            retained_before_clear,
                            total_before,
                            total_before
                        );
                    }
                }
            }

            // Refresh updated_at so least-recently-used routing stays meaningful.
            let _ = state.upsert_route(string_key, &route.session_id);
            debug!(
                "resumed gateway session {} for {string_key} ({} message(s))",
                route.session_id,
                messages.len()
            );
            return GatewaySession::from_persisted(
                key.clone(),
                route.session_id,
                messages,
                model,
                route.voice_mode,
            );
        }

        // Fresh session — write-through to SQLite, then record the durable route.
        let session = GatewaySession::new(key.clone(), model);
        if let Err(e) = state.create_session(
            &session.session_id,
            source,
            Some(model),
            None, // system_prompt set later
            None, // no parent
            workspace_root.as_deref(),
        ) {
            warn!("Failed to persist session to SQLite: {e}");
        }
        if self.persist_sessions
            && let Err(e) = state.upsert_route(string_key, &session.session_id)
        {
            warn!("Failed to persist gateway route: {e}");
        }
        session
    }

    /// Phase 36.17.9: set the voice mode (`off` | `on` | `tts`) for a chat,
    /// write-through to the durable `gateway_routes` row so it survives restart.
    /// Updates the in-memory session when present. Returns true if an in-memory
    /// session was updated (false = persisted only, no live session yet).
    pub fn set_voice_mode(&mut self, key: &SessionKey, mode: &str) -> bool {
        let string_key = key.to_string_key();
        if self.persist_sessions
            && let Ok(mut state) = self.state.lock()
            && let Err(e) = state.set_route_voice_mode(&string_key, mode)
        {
            warn!("Failed to persist voice mode: {e}");
        }
        if let Some(session) = self.sessions.get_mut(&string_key) {
            session.voice_mode = mode.to_string();
            true
        } else {
            false
        }
    }

    /// Phase 36.17.9: current voice mode for a chat, if an in-memory session exists.
    pub fn voice_mode(&self, key: &SessionKey) -> Option<String> {
        self.sessions
            .get(&key.to_string_key())
            .map(|s| s.voice_mode.clone())
    }

    /// Add a message to both the in-memory cache and SQLite.
    /// Per D-01: write-through on every message.
    pub fn add_message_to_session(&mut self, key: &SessionKey, msg: ChatMessage) -> bool {
        let string_key = key.to_string_key();
        if let Some(session) = self.sessions.get_mut(&string_key) {
            // Write-through to SQLite
            if let Ok(mut state) = self.state.lock()
                && let Err(e) = state.add_message(&session.session_id, &msg)
            {
                warn!("Failed to persist message to SQLite: {e}");
            }
            session.add_message(msg);
            true
        } else {
            false
        }
    }

    /// Phase 39.1 (R39.1-02, D-02): Atomically append a batch of messages to the
    /// in-memory history and write them through to SQLite under a SINGLE state-lock
    /// acquisition.
    ///
    /// This is the turn-completion append site.  All messages are persisted as a unit
    /// — no concurrent turn can interleave its own messages between ours.  The
    /// `std::sync::Mutex` on `state` is held only for the duration of the SQLite
    /// writes, never across an `.await`.
    ///
    /// Returns `true` if the session was found (messages appended), `false` if the
    /// session has already been removed from `SessionStore` (e.g. by a concurrent
    /// `/new`).  When `false`, callers should fall back to the `Arc<Mutex<Vec>>`
    /// handle obtained from [`GatewaySession::history_arc`] at turn-start, which
    /// keeps the Vec alive via refcount (RESEARCH Pitfall 3).
    pub fn add_messages_batch_to_session(
        &mut self,
        key: &SessionKey,
        messages: Vec<ChatMessage>,
    ) -> bool {
        let string_key = key.to_string_key();
        if let Some(session) = self.sessions.get_mut(&string_key) {
            // Write all messages through to SQLite under ONE lock acquisition.
            // Lock never held across .await (this fn is sync).
            if let Ok(mut state) = self.state.lock() {
                for msg in &messages {
                    if let Err(e) = state.add_message(&session.session_id, msg) {
                        warn!("Failed to persist message in batch to SQLite: {e}");
                    }
                }
            }
            // Update in-memory history (both Vec<ChatMessage> and Arc<Mutex<Vec>>).
            for msg in messages {
                session.add_message(msg);
            }
            true
        } else {
            false
        }
    }

    /// Get a reference to the underlying StateStore (for direct queries, WAL checkpoint, etc.)
    pub fn state_store(&self) -> &Arc<Mutex<StateStore>> {
        &self.state
    }

    pub fn get(&self, key: &SessionKey) -> Option<&GatewaySession> {
        self.sessions.get(&key.to_string_key())
    }

    /// Phase 36 (D-03/D-05/D-06): return the per-session running flag Arc.
    ///
    /// If the session exists, returns a clone of its `running` field.
    /// If the session does not yet exist (first message of a chat), returns a fresh
    /// `Arc::new(AtomicBool::new(false))` — no agent turn is in-flight yet, so
    /// false is the correct value. The caller can hand this to `CommandContext`;
    /// subsequent messages will find the real session entry once `run_agent`
    /// calls `get_or_create`.
    pub fn get_running_flag(&self, key: &SessionKey) -> Arc<AtomicBool> {
        self.sessions
            .get(&key.to_string_key())
            .map(|s| s.running.clone())
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)))
    }

    pub fn get_mut(&mut self, key: &SessionKey) -> Option<&mut GatewaySession> {
        self.sessions.get_mut(&key.to_string_key())
    }

    /// Phase 47.5 (D-04): the DURABLE session reset —
    /// 34b in-memory discard PLUS end_session + route clear, resolving the target session
    /// from the durable route when nothing is in memory (post-restart /new). Returns whether
    /// a session existed in either source. voice_mode is preserved (D-VOICE).
    pub fn reset_session(&mut self, key: &SessionKey, end_reason: &str) -> bool {
        let string_key = key.to_string_key();
        let removed = self.sessions.remove(&string_key);

        // Leak cleanup: drop the cached trajectory writer keyed by the old
        // session UUID — the fresh session (once minted) gets a new one.
        if let Some(session) = &removed {
            self.trajectory_writers.remove(&session.session_id);
        }

        let Ok(mut state) = self.state.lock() else {
            warn!(
                "state lock poisoned — reset_session degraded to in-memory-only for {string_key}"
            );
            return removed.is_some();
        };

        // One read serves both the durable-only fallback and D-VOICE.
        let route = state.get_route(&string_key).ok().flatten();

        // Resolve the target session id: the removed in-memory session's id if
        // present, otherwise the route's session id when non-empty. The route
        // branch is the review's consensus-HIGH fix — post-restart, the
        // in-memory map is empty and the route is the only handle on the
        // session that `/new` must end.
        let target_id = removed.as_ref().map(|s| s.session_id.clone()).or_else(|| {
            route
                .as_ref()
                .filter(|r| !r.session_id.is_empty())
                .map(|r| r.session_id.clone())
        });

        let target_id_resolved_from_route = target_id.is_some();

        if let Some(id) = &target_id
            && let Err(e) = state.end_session(id, end_reason)
        {
            warn!("Failed to end session {id} during reset: {e}");
        }

        if let Err(e) = state.delete_route(&string_key) {
            warn!("Failed to delete route for {string_key} during reset: {e}");
        }

        // D-VOICE: a non-default voice mode is a chat preference, not session
        // content — re-apply it as the resume-inert empty-session_id
        // placeholder row that delete_route just cleared away.
        if let Some(route) = &route
            && route.voice_mode != "off"
            && let Err(e) = state.set_route_voice_mode(&string_key, &route.voice_mode)
        {
            warn!("Failed to re-apply voice_mode for {string_key} during reset: {e}");
        }

        removed.is_some() || target_id_resolved_from_route
    }

    pub fn list(&self) -> Vec<&GatewaySession> {
        self.sessions.values().collect()
    }

    pub fn count(&self) -> usize {
        self.sessions.len()
    }

    /// Remove all sessions that have been inactive longer than `timeout_hours`.
    /// Also ends expired sessions in SQLite.
    pub fn expire_stale(&mut self, timeout_hours: u64) {
        let before = self.sessions.len();
        let cutoff = Utc::now() - chrono::Duration::hours(timeout_hours as i64);
        let expired_keys: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.updated_at < cutoff)
            .map(|(k, _)| k.clone())
            .collect();

        for key in &expired_keys {
            if let Some(session) = self.sessions.remove(key) {
                // End session in SQLite
                if let Ok(mut state) = self.state.lock() {
                    let _ = state.end_session(&session.session_id, "expired");
                }
            }
        }

        let evicted = before - self.sessions.len();
        if evicted > 0 {
            debug!("Evicted {} stale session(s)", evicted);
        }
    }
}

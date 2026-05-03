use std::sync::{Arc, Mutex};

use ironhermes_core::{ChatMessage, Platform};
use ironhermes_state::StateStore;
use std::collections::HashMap;
use chrono::{DateTime, Utc};
use tracing::{debug, warn};

/// Unique key for a gateway session (platform + chat_id + optional user_id).
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SessionKey {
    pub platform: Platform,
    pub chat_id: String,
    pub user_id: Option<String>,
}

impl SessionKey {
    pub fn new(platform: Platform, chat_id: impl Into<String>) -> Self {
        Self {
            platform,
            chat_id: chat_id.into(),
            user_id: None,
        }
    }

    pub fn with_user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    pub fn to_string_key(&self) -> String {
        match &self.user_id {
            Some(uid) => format!("{}:{}:{}", self.platform, self.chat_id, uid),
            None => format!("{}:{}", self.platform, self.chat_id),
        }
    }
}

/// An active gateway conversation session.
#[derive(Debug, Clone)]
pub struct GatewaySession {
    pub key: SessionKey,
    pub session_id: String,
    pub messages: Vec<ChatMessage>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: String,
}

impl GatewaySession {
    pub fn new(key: SessionKey, model: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            key,
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
            model: model.into(),
        }
    }

    pub fn add_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
        self.updated_at = Utc::now();
    }

    pub fn clear(&mut self) {
        self.messages.clear();
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
}

/// Write-through session cache: SQLite persistence via StateStore + in-memory HashMap for fast access.
/// Per D-01: every create/add_message writes to SQLite immediately.
/// Per D-02: on restart, fresh session starts — old data is query-only via StateStore.
pub struct SessionStore {
    state: Arc<Mutex<StateStore>>,
    sessions: HashMap<String, GatewaySession>,
}

impl SessionStore {
    pub fn new(state: Arc<Mutex<StateStore>>) -> Self {
        Self {
            state,
            sessions: HashMap::new(),
        }
    }

    /// Get or create a session for the given key. On creation, writes through to SQLite.
    pub fn get_or_create(&mut self, key: SessionKey, model: &str, source: &str) -> &mut GatewaySession {
        let string_key = key.to_string_key();
        if !self.sessions.contains_key(&string_key) {
            let session = GatewaySession::new(key.clone(), model);
            // Write-through: persist to SQLite immediately
            if let Ok(mut state) = self.state.lock() {
                if let Err(e) = state.create_session(
                    &session.session_id,
                    source,
                    Some(model),
                    None, // system_prompt set later
                    None, // no parent
                    None, // workspace_root: Plan 0 placeholder — Plan 8 wires resolved workspace
                ) {
                    warn!("Failed to persist session to SQLite: {e}");
                }
            }
            self.sessions.insert(string_key.clone(), session);
        }
        self.sessions.get_mut(&string_key).unwrap()
    }

    /// Add a message to both the in-memory cache and SQLite.
    /// Per D-01: write-through on every message.
    pub fn add_message_to_session(&mut self, key: &SessionKey, msg: ChatMessage) -> bool {
        let string_key = key.to_string_key();
        if let Some(session) = self.sessions.get_mut(&string_key) {
            // Write-through to SQLite
            if let Ok(mut state) = self.state.lock() {
                if let Err(e) = state.add_message(&session.session_id, &msg) {
                    warn!("Failed to persist message to SQLite: {e}");
                }
            }
            session.add_message(msg);
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

    pub fn get_mut(&mut self, key: &SessionKey) -> Option<&mut GatewaySession> {
        self.sessions.get_mut(&key.to_string_key())
    }

    pub fn remove(&mut self, key: &SessionKey) -> Option<GatewaySession> {
        self.sessions.remove(&key.to_string_key())
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
        let expired_keys: Vec<String> = self.sessions
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

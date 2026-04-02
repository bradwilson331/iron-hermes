use ironhermes_core::{ChatMessage, Platform};
use std::collections::HashMap;
use chrono::{DateTime, Utc};

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

/// In-memory session store for the gateway.
#[derive(Debug, Default)]
pub struct SessionStore {
    sessions: HashMap<String, GatewaySession>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_create(&mut self, key: SessionKey, model: &str) -> &mut GatewaySession {
        let string_key = key.to_string_key();
        self.sessions
            .entry(string_key)
            .or_insert_with(|| GatewaySession::new(key, model))
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
    pub fn expire_stale(&mut self, timeout_hours: u64) {
        let cutoff = Utc::now() - chrono::Duration::hours(timeout_hours as i64);
        self.sessions.retain(|_, session| session.updated_at > cutoff);
    }
}

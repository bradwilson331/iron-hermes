//! Cross-consumer session key (Phase 36.17.3, D-01 / Resolution 3).
//!
//! `SessionKey` was originally defined in `ironhermes-gateway::session` for the
//! gateway's per-chat queue. Phase 36.17.3 relocates it here so every
//! `MessageQueue<K>` consumer (TUI, future web/Discord/Slack/REST) shares the
//! same canonical key without taking a hard gateway dependency. The gateway
//! re-exports this type from `ironhermes_gateway::session` for back-compat —
//! existing `use ironhermes_gateway::session::SessionKey;` call sites continue
//! to compile unchanged (Resolution 3, single move + re-export).
//!
//! `Platform` already lives in `ironhermes-core::types`, so the relocation is
//! dep-free.

use crate::Platform;

/// Unique key for a gateway/TUI/web session.
///
/// Combination of `platform`, `chat_id`, and optional `user_id` uniquely
/// identifies a conversational thread. The optional `user_id` lets two
/// distinct users share the same `chat_id` (e.g., a Telegram group chat) and
/// still get independent queues — primary mitigation for the cross-session
/// leak surface (T-36.17.1-02 from the parent phase).
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SessionKey {
    pub platform: Platform,
    pub chat_id: String,
    pub user_id: Option<String>,
}

impl SessionKey {
    /// Construct a `SessionKey` with no `user_id` (DM / single-user channel).
    pub fn new(platform: Platform, chat_id: impl Into<String>) -> Self {
        Self {
            platform,
            chat_id: chat_id.into(),
            user_id: None,
        }
    }

    /// Attach a `user_id` to disambiguate users sharing the same `chat_id`
    /// (e.g., group chats). Returns `self` for ergonomic chaining.
    pub fn with_user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Canonical string form used for logging, hash-keyed map lookups in
    /// downstream stores, and `QueueError::CapacityReached.session_key`.
    pub fn to_string_key(&self) -> String {
        match &self.user_id {
            Some(uid) => format!("{}:{}:{}", self.platform, self.chat_id, uid),
            None => format!("{}:{}", self.platform, self.chat_id),
        }
    }
}

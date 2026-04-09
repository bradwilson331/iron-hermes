use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Safely truncate a string at a UTF-8 character boundary.
/// Mirrors the floor_char_boundary pattern from crates/ironhermes-cron/src/delivery.rs.
pub fn preview(content: &str, max_len: usize) -> String {
    if content.len() <= max_len {
        content.to_string()
    } else {
        let safe_end = content.floor_char_boundary(max_len);
        content[..safe_end].to_string()
    }
}

/// The kind of hook event, tagged with `"kind"` in serialized JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookEventKind {
    /// A message was received from a platform.
    MessageReceived {
        platform: String,
        chat_id: String,
        content_preview: String,
    },
    /// A tool was invoked by the agent.
    ToolCalled {
        tool_name: String,
        args_preview: String,
    },
    /// A tool completed execution (success or failure).
    ToolCompleted {
        tool_name: String,
        success: bool,
        result_preview: String,
        duration_ms: u64,
    },
    /// The agent sent a response to a platform.
    ResponseSent {
        platform: String,
        chat_id: String,
        response_preview: String,
    },
    /// A skill was activated (loaded for use by agent or cron).
    SkillActivated {
        skill_name: String,
        /// Source of activation: "tool" or "cron"
        source: String,
    },
}

/// A single observable event emitted by the agent at a lifecycle point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEvent {
    /// Unique event ID (UUID v4).
    pub id: String,
    /// Per-request correlation ID shared across all events in a single agent run.
    pub request_id: String,
    /// UTC timestamp when the event was created.
    pub timestamp: DateTime<Utc>,
    /// The event kind and associated data.
    #[serde(flatten)]
    pub kind: HookEventKind,
}

impl HookEvent {
    /// Create a new HookEvent for the given request_id and kind.
    /// Generates a fresh UUID for `id` and captures `Utc::now()` for `timestamp`.
    pub fn new(request_id: &str, kind: HookEventKind) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            request_id: request_id.to_string(),
            timestamp: Utc::now(),
            kind,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_event_serialization() {
        let event = HookEvent::new(
            "req-123",
            HookEventKind::MessageReceived {
                platform: "telegram".to_string(),
                chat_id: "42".to_string(),
                content_preview: "hello".to_string(),
            },
        );

        let json = serde_json::to_string(&event).expect("serialize");
        // The flattened tag should appear in the JSON
        assert!(json.contains("\"kind\""), "kind field must be present: {json}");
        assert!(json.contains("message_received"), "kind value must be present: {json}");
        assert!(json.contains("req-123"), "request_id must be present: {json}");

        // Round-trip
        let decoded: HookEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.request_id, "req-123");
        match decoded.kind {
            HookEventKind::MessageReceived { platform, .. } => {
                assert_eq!(platform, "telegram");
            }
            other => panic!("unexpected kind: {other:?}"),
        }
    }

    #[test]
    fn test_preview_truncation() {
        // ASCII
        let s = "a".repeat(300);
        let p = preview(&s, 200);
        assert_eq!(p.len(), 200);

        // Multi-byte UTF-8: each char is 3 bytes
        let s = "日".repeat(100); // 300 bytes total
        let p = preview(&s, 200);
        // floor_char_boundary(200) on 3-byte chars: 200/3=66 chars = 198 bytes
        assert!(p.len() <= 200);
        assert!(p.is_empty() || std::str::from_utf8(p.as_bytes()).is_ok());
    }

    #[test]
    fn test_preview_short_string_unchanged() {
        let s = "hello world";
        let p = preview(s, 200);
        assert_eq!(p, s);
    }

    #[test]
    fn test_all_event_kinds_serialize() {
        let kinds = vec![
            HookEventKind::MessageReceived {
                platform: "telegram".to_string(),
                chat_id: "1".to_string(),
                content_preview: "msg".to_string(),
            },
            HookEventKind::ToolCalled {
                tool_name: "bash".to_string(),
                args_preview: "{}".to_string(),
            },
            HookEventKind::ToolCompleted {
                tool_name: "bash".to_string(),
                success: true,
                result_preview: "ok".to_string(),
                duration_ms: 42,
            },
            HookEventKind::ResponseSent {
                platform: "telegram".to_string(),
                chat_id: "1".to_string(),
                response_preview: "done".to_string(),
            },
            HookEventKind::SkillActivated {
                skill_name: "focus".to_string(),
                source: "cron".to_string(),
            },
        ];

        for kind in kinds {
            let event = HookEvent::new("req", kind);
            let json = serde_json::to_string(&event).expect("serialize");
            assert!(json.contains("\"kind\""), "missing kind tag: {json}");
        }
    }
}

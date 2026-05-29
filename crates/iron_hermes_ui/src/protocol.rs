//! Shared protocol types for WebSocket chat.
//!
//! These types are used on BOTH client and server — they are simple
//! serializable data structures with no server-only dependencies.
//! Kept in a separate unconditional module so the WASM client can
//! compile them without pulling in the `server` feature.

use serde::{Deserialize, Serialize};

/// Client → server message (user input).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChatRequest {
    pub session_id: String,
    pub message: String,
}

/// Server → client streaming events.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ChatStreamEvent {
    /// Streaming text delta from the agent.
    Delta { text: String },
    /// Agent started a tool call.
    ToolCallStart { name: String, args: String },
    /// Tool call completed.
    ToolCallEnd { name: String, success: bool },
    /// Agent response finished.
    Finished { total_tokens: u32 },
    /// Error during agent execution.
    Error { message: String },
    /// Phase 26.7.1 Plan 02 (D-07): payload-free subagent-registry-changed signal.
    ///
    /// JSON shape (external tagging — no #[serde(...)] attribute on the enum):
    ///   {"SubagentEvent":{}}
    ///
    /// Client increments `subagent_events: Signal<u64>` on receipt and lets
    /// `ScreenAgents`' use_effect call `agents_resource.restart()` — same code
    /// path as the periodic poll, no divergent diff logic.
    SubagentEvent {},
    /// Phase 36.17.4 (D-03): queue depth + paused state snapshot. JSON shape
    /// (external tagging): {"QueueUpdated":{"depth":3,"paused":false}}. Emitted
    /// on every push, pop, pause toggle, unpause, and queue clear. Client
    /// updates Signal<(u32, bool)> for the status-bar Queue: N pill.
    QueueUpdated { depth: u32, paused: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase 26.7.1 Plan 02 (Wave 0): D-07 serde shape verification.
    /// SubagentEvent {} must serialize to {"SubagentEvent":{}} (external tagging).
    #[test]
    fn test_subagent_event_json_shape() {
        let ev = ChatStreamEvent::SubagentEvent {};
        let json = serde_json::to_string(&ev).expect("serialize SubagentEvent");
        assert_eq!(json, r#"{"SubagentEvent":{}}"#);

        // Round-trip: deserialize back into the variant.
        let parsed: ChatStreamEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(
            matches!(parsed, ChatStreamEvent::SubagentEvent {}),
            "round-trip must reconstruct SubagentEvent variant"
        );
    }

    /// Phase 36.17.4 Plan 02 (D-11): QueueUpdated wire-format lock.
    /// External-tagged struct variant must serialize to
    /// {"QueueUpdated":{"depth":3,"paused":false}}.
    /// Both paused=false and paused=true cases asserted; round-trip preserved.
    #[test]
    fn test_queue_updated_json_shape() {
        // depth=3, paused=false: literal wire-format lock.
        let ev = ChatStreamEvent::QueueUpdated {
            depth: 3,
            paused: false,
        };
        let json = serde_json::to_string(&ev).expect("serialize QueueUpdated");
        assert_eq!(
            json, r#"{"QueueUpdated":{"depth":3,"paused":false}}"#,
            "D-11: QueueUpdated must serialize to external-tagged struct shape"
        );

        // Round-trip: deserialize back into the variant.
        let parsed: ChatStreamEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(
            matches!(
                parsed,
                ChatStreamEvent::QueueUpdated {
                    depth: 3,
                    paused: false,
                }
            ),
            "round-trip must reconstruct QueueUpdated {{ depth: 3, paused: false }}"
        );

        // paused=true variant: separate shape lock.
        let ev_paused = ChatStreamEvent::QueueUpdated {
            depth: 7,
            paused: true,
        };
        let json_paused =
            serde_json::to_string(&ev_paused).expect("serialize paused QueueUpdated");
        assert_eq!(
            json_paused, r#"{"QueueUpdated":{"depth":7,"paused":true}}"#,
            "D-11: paused=true variant must serialize correctly"
        );
    }
}

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

// =============================================================================
// Phase 36.3.7.11 Plan 01 — kanban dashboard wire types
// =============================================================================

/// Phase 36.3.7.11 (D-08): kanban dashboard WebSocket envelope.
///
/// External tagging:
/// - `{"TaskEventBatch":{"events":[...],"last_event_id":42}}`
/// - `{"Error":{"message":"..."}}`
/// - `{"Ping":{}}`
///
/// Server → client only. The dashboard tail consumer pushes one
/// `TaskEventBatch` per polling cycle that observed new events.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum KanbanWsEvent {
    /// Batch of new `task_events` rows since the last broadcast.
    /// `last_event_id` is the highest `id` in `events` — used by the
    /// client as a reconnect cursor (Plan 02 behavior).
    TaskEventBatch {
        events: Vec<KanbanEventRow>,
        last_event_id: i64,
    },
    /// Server-side error encountered while streaming (rendered as a toast).
    Error { message: String },
    /// Payload-free liveness ping (mirrors ChatStreamEvent::SubagentEvent {}).
    Ping {},
}

/// Phase 36.3.7.11 (D-08): one row from the `task_events` table on the wire.
///
/// Plain Serde struct — no server-only deps. Mirrors
/// `ironhermes_kanban::events::KanbanEvent` but lives in protocol.rs so the
/// WASM client can compile it without pulling `ironhermes-kanban` into the
/// client build (Pattern A in PATTERNS.md).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct KanbanEventRow {
    pub id: i64,
    pub task_id: String,
    pub kind: String,
    pub payload: Option<String>,
    pub created_at: f64,
}

/// Phase 36.3.7.11 (D-13 / Q6): UI-layer transport wrapper for the
/// Complete / Block structured input. Lives here (not in `ironhermes-kanban`)
/// because it is a UI-layer concept — the dashboard's `patch_task_status`
/// `#[server]` fn carries this in lieu of separate `complete` / `block` write
/// fns. Plan 02 wires this up; Plan 01 just defines the type so the wire
/// contract is stable.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum PromptPayload {
    Complete {
        summary: String,
        metadata: Option<serde_json::Value>,
    },
    Block {
        reason: String,
    },
}

/// Phase 36.3.7.11 (D-19): board task on the wire. Mirrors
/// `ironhermes_kanban::types::Task` for read-side rendering. Plain Serde —
/// no server-only deps so the WASM client can render it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    pub body: Option<String>,
    pub assignee: String,
    pub status: String,
    pub priority: i64,
    pub tenant: Option<String>,
    pub workspace: Option<String>,
    pub created_at: f64,
    pub started_at: Option<f64>,
    pub ended_at: Option<f64>,
}

/// Phase 36.3.7.11 (Q5): the canonical worker_context contract returned by
/// `fetch_task`. Field set mirrors `kanban_show.rs` lines 218-232 exactly so
/// the drawer renders the same shape the LLM `kanban_show` tool produces.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WorkerContextEnvelope {
    pub task_id: String,
    pub title: String,
    pub body: Option<String>,
    pub status: String,
    pub assignee: String,
    pub tenant: Option<String>,
    pub workspace: String,
    pub priority: i64,
    pub parent_handoffs: Vec<serde_json::Value>,
    pub prior_attempts: Vec<serde_json::Value>,
    pub comments: Vec<serde_json::Value>,
}

/// Phase 36.3.7.11 (D-20): one `task_runs` row on the wire. Used by the
/// drawer Run History section.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TaskRunRow {
    pub run_id: String,
    pub outcome: Option<String>,
    pub started_at: f64,
    pub ended_at: Option<f64>,
    pub elapsed_ms: Option<i64>,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub worker: Option<String>,
}

/// Phase 36.3.7.11 (D-20): one `task_comments` row on the wire. Used by the
/// drawer comment thread.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CommentRow {
    pub author: String,
    pub body: String,
    pub created_at: f64,
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

    // =========================================================================
    // Phase 36.3.7.11 Plan 01 (D-08 / D-13 / Q6) — Kanban wire-format locks
    // =========================================================================

    /// Phase 36.3.7.11 (D-08): TaskEventBatch must serialize as
    /// `{"TaskEventBatch":{"events":[...],"last_event_id":N}}`.
    /// Round-trip preserves field shape.
    #[test]
    fn test_kanban_ws_event_task_event_batch_json_shape() {
        let ev = KanbanWsEvent::TaskEventBatch {
            events: vec![],
            last_event_id: 42,
        };
        let json = serde_json::to_string(&ev).expect("serialize TaskEventBatch");
        assert!(
            json.starts_with(r#"{"TaskEventBatch":"#),
            "D-08: TaskEventBatch must use external tagging (got {json})"
        );
        assert!(
            json.contains(r#""last_event_id":42"#),
            "D-08: TaskEventBatch must serialize last_event_id (got {json})"
        );
        // Round-trip.
        let parsed: KanbanWsEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(
            matches!(
                parsed,
                KanbanWsEvent::TaskEventBatch {
                    last_event_id: 42,
                    ..
                }
            ),
            "round-trip must reconstruct TaskEventBatch variant"
        );
    }

    /// Phase 36.3.7.11 (D-08): Ping is the payload-free liveness variant.
    /// External-tag struct-variant serialization: `{"Ping":{}}`.
    #[test]
    fn test_kanban_ws_event_ping_json_shape() {
        let ev = KanbanWsEvent::Ping {};
        let json = serde_json::to_string(&ev).expect("serialize Ping");
        assert_eq!(
            json, r#"{"Ping":{}}"#,
            "D-08: Ping must serialize to {{\"Ping\":{{}}}}"
        );
    }

    /// Phase 36.3.7.11 (D-13 / Q6): PromptPayload::Complete round-trip.
    #[test]
    fn test_prompt_payload_complete_round_trip() {
        let ev = PromptPayload::Complete {
            summary: "shipped".to_string(),
            metadata: Some(serde_json::json!({ "pr": 123 })),
        };
        let json = serde_json::to_string(&ev).expect("serialize Complete");
        let parsed: PromptPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, ev, "D-13: Complete must round-trip");
    }

    /// Phase 36.3.7.11 (D-13 / Q6): PromptPayload::Block round-trip.
    #[test]
    fn test_prompt_payload_block_round_trip() {
        let ev = PromptPayload::Block {
            reason: "waiting on dependency".to_string(),
        };
        let json = serde_json::to_string(&ev).expect("serialize Block");
        let parsed: PromptPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, ev, "D-13: Block must round-trip");
    }
}

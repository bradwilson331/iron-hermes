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
    /// Phase 36.17.7 D-02-a: synthesized audio delivery to the web client.
    /// JSON wire shape (external tagging):
    ///   {"AudioOut":{"mime":"audio/mpeg","uuid":"<uuidv4>","bytes":[<u8>...]}}
    /// Transmitted as Message::Binary per D-02-a. The `bytes` field serializes
    /// as a JSON array of u8 values (no `serde_bytes` dep — plain Vec<u8>).
    AudioOut { mime: String, uuid: String, bytes: Vec<u8> },
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
#[allow(dead_code)] // Plan 02 consumes this via the write-side `#[server]` fns.
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

// =============================================================================
// Phase 36.3.7.11 Plan 02 — kanban write-side wire types
// =============================================================================

/// Phase 36.3.7.11 Plan 02 (D-09 wire): wire-copy of
/// `ironhermes_kanban::types::KanbanStatus`. Plain Serde enum so the WASM
/// client can match on a status without pulling `ironhermes-kanban` into the
/// client build (Pattern A in PATTERNS.md). Variant set MUST match the
/// canonical seven variants byte-for-byte by `as_str` casing — the
/// `rename_all = "lowercase"` attribute combined with the special-case
/// `InProgress → "running"` mapping (via `#[serde(rename)]`) matches
/// `KanbanStatus::as_str` exactly (`triage`, `todo`, `ready`, `running`,
/// `blocked`, `done`, `archived`).
///
/// The client-side `kanban::transitions` module uses this enum; the
/// server-side `kanban_api::patch_task_status` parses the inbound value
/// then forwards to `ironhermes_kanban::KanbanStatus` for the actual store
/// call. The drift-risk surface is the variant set + the wire string for
/// `InProgress`/Running; both are locked by inline tests below.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum KanbanStatus {
    Triage,
    Todo,
    Ready,
    /// Wire form is "running" (matches `ironhermes_kanban::KanbanStatus::Running`).
    #[serde(rename = "running")]
    InProgress,
    Blocked,
    Done,
    Archived,
}

impl KanbanStatus {
    /// Canonical lowercase wire string (matches
    /// `ironhermes_kanban::KanbanStatus::as_str`).
    pub fn as_wire_str(self) -> &'static str {
        match self {
            KanbanStatus::Triage => "triage",
            KanbanStatus::Todo => "todo",
            KanbanStatus::Ready => "ready",
            KanbanStatus::InProgress => "running",
            KanbanStatus::Blocked => "blocked",
            KanbanStatus::Done => "done",
            KanbanStatus::Archived => "archived",
        }
    }

    /// Parse from the canonical lowercase wire string. Returns `None` for
    /// unknown values — callers should reject with a `ServerFnError`.
    pub fn from_wire_str(s: &str) -> Option<Self> {
        Some(match s {
            "triage" => KanbanStatus::Triage,
            "todo" => KanbanStatus::Todo,
            "ready" => KanbanStatus::Ready,
            "running" => KanbanStatus::InProgress,
            "blocked" => KanbanStatus::Blocked,
            "done" => KanbanStatus::Done,
            "archived" => KanbanStatus::Archived,
            _ => return None,
        })
    }
}

/// Phase 36.3.7.11 Plan 02 (D-13): payload for `create_task` `#[server]` fn.
/// Carries the structured form input from the dashboard's Create Task modal
/// to the server-side `KanbanStore::create_task` call.
///
/// Round-trips through serde — see inline tests below.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct CreateTaskPayload {
    pub title: String,
    pub assignee: Option<String>,
    pub parents: Vec<String>,
    pub priority: i64,
    pub tenant: Option<String>,
    pub body: Option<String>,
    /// When `true`, the new task starts in TRIAGE; when `false`, it starts
    /// in TODO (or READY if parents.is_empty(), per the store's
    /// `create_task` D-06 rule).
    pub start_in_triage: bool,
}

/// Phase 36.3.7.11 Plan 02 (D-13): which decomposer kernel action to invoke.
/// External-tag wire shape (default Rust enum Serde): bare strings
/// `"Decompose"` / `"Specify"`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecomposeOrSpecify {
    Decompose,
    Specify,
}

impl DecomposeOrSpecify {
    /// Lowercase action slug for CLI hint copy ("decompose" / "specify").
    pub fn slug(self) -> &'static str {
        match self {
            DecomposeOrSpecify::Decompose => "decompose",
            DecomposeOrSpecify::Specify => "specify",
        }
    }
}

/// Phase 36.3.7.11 Plan 02 (D-13 / Q9): result of `run_decompose_or_specify`
/// `#[server]` fn. Two branches:
///
/// - `Ok` — the kernel ran. Returns the number of children spawned (0 for
///   the specify path) and a short human-readable summary.
/// - `NotWired` — the dashboard's AppState does not satisfy the kernel's
///   aux-client requirement. The UI surfaces the message as a tooltip and
///   the user runs the CLI command directly.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum DecomposeResult {
    Ok {
        children_count: u32,
        summary: String,
    },
    NotWired {
        message: String,
    },
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

    /// Phase 36.17.7 D-02-a: AudioOut wire-format lock.
    /// External-tagged struct variant must serialize to
    /// {"AudioOut":{"mime":"audio/mpeg","uuid":"test-uuid","bytes":[255,251]}}.
    /// Round-trip preserved via serde_json.
    #[test]
    fn test_audio_out_json_shape() {
        let ev = ChatStreamEvent::AudioOut {
            mime: "audio/mpeg".to_string(),
            uuid: "test-uuid".to_string(),
            bytes: vec![0xFF, 0xFB],
        };
        let json = serde_json::to_string(&ev).expect("serialize AudioOut");
        assert!(
            json.starts_with(r#"{"AudioOut":"#),
            "D-02-a: AudioOut must use external tagging (got {json})"
        );
        assert!(
            json.contains(r#""mime":"audio/mpeg""#),
            "D-02-a: AudioOut must serialize mime field (got {json})"
        );
        assert!(
            json.contains(r#""uuid":"test-uuid""#),
            "D-02-a: AudioOut must serialize uuid field (got {json})"
        );
        // Round-trip.
        let parsed: ChatStreamEvent = serde_json::from_str(&json).expect("deserialize AudioOut");
        assert!(
            matches!(parsed, ChatStreamEvent::AudioOut { .. }),
            "D-02-a: AudioOut must round-trip via serde_json"
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

    // =========================================================================
    // Phase 36.3.7.11 Plan 02 (D-09 wire / D-13) — kanban write-side types
    // =========================================================================

    /// D-09 wire: every `KanbanStatus` variant serializes to the canonical
    /// lowercase string (matches `ironhermes_kanban::KanbanStatus::as_str`).
    /// InProgress maps to "running".
    #[test]
    fn test_kanban_status_serializes_lowercase() {
        let cases = [
            (KanbanStatus::Triage, "\"triage\""),
            (KanbanStatus::Todo, "\"todo\""),
            (KanbanStatus::Ready, "\"ready\""),
            (KanbanStatus::InProgress, "\"running\""),
            (KanbanStatus::Blocked, "\"blocked\""),
            (KanbanStatus::Done, "\"done\""),
            (KanbanStatus::Archived, "\"archived\""),
        ];
        for (variant, expected) in cases {
            let json = serde_json::to_string(&variant).expect("serialize KanbanStatus");
            assert_eq!(
                json, expected,
                "D-09 wire: {:?} must serialize as {}",
                variant, expected
            );
            let parsed: KanbanStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, variant, "D-09 wire: round-trip preserves variant");
        }
    }

    /// D-09 wire: as_wire_str helper agrees with serde shape.
    #[test]
    fn test_kanban_status_as_wire_str_matches_serde() {
        for v in [
            KanbanStatus::Triage,
            KanbanStatus::Todo,
            KanbanStatus::Ready,
            KanbanStatus::InProgress,
            KanbanStatus::Blocked,
            KanbanStatus::Done,
            KanbanStatus::Archived,
        ] {
            let serde_str = serde_json::to_string(&v).unwrap();
            assert_eq!(
                serde_str,
                format!("\"{}\"", v.as_wire_str()),
                "as_wire_str must match serde for {:?}",
                v
            );
        }
    }

    /// D-09 wire: from_wire_str inverse-of as_wire_str.
    #[test]
    fn test_kanban_status_from_wire_str_round_trip() {
        for v in [
            KanbanStatus::Triage,
            KanbanStatus::Todo,
            KanbanStatus::Ready,
            KanbanStatus::InProgress,
            KanbanStatus::Blocked,
            KanbanStatus::Done,
            KanbanStatus::Archived,
        ] {
            let parsed = KanbanStatus::from_wire_str(v.as_wire_str()).expect("parse");
            assert_eq!(parsed, v);
        }
        assert!(KanbanStatus::from_wire_str("nonsense").is_none());
    }

    /// D-13: `CreateTaskPayload` round-trips through serde.
    #[test]
    fn test_create_task_payload_round_trip() {
        let payload = CreateTaskPayload {
            title: "wire up dashboard".to_string(),
            assignee: Some("frontend-dev".to_string()),
            parents: vec!["t_a".to_string(), "t_b".to_string()],
            priority: 2,
            tenant: Some("dashboard".to_string()),
            body: Some("acceptance: drag works".to_string()),
            start_in_triage: true,
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        let parsed: CreateTaskPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, payload, "D-13: CreateTaskPayload must round-trip");
    }

    /// D-13: `DecomposeOrSpecify` round-trips + slug helper.
    #[test]
    fn test_decompose_or_specify_round_trip() {
        for v in [DecomposeOrSpecify::Decompose, DecomposeOrSpecify::Specify] {
            let json = serde_json::to_string(&v).expect("serialize");
            let parsed: DecomposeOrSpecify =
                serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, v, "D-13: DecomposeOrSpecify must round-trip");
        }
        // External-tag bare-string shape.
        assert_eq!(
            serde_json::to_string(&DecomposeOrSpecify::Decompose).unwrap(),
            r#""Decompose""#
        );
        assert_eq!(DecomposeOrSpecify::Decompose.slug(), "decompose");
        assert_eq!(DecomposeOrSpecify::Specify.slug(), "specify");
    }

    /// D-13 / Q9: `DecomposeResult` round-trips for both branches.
    #[test]
    fn test_decompose_result_round_trip() {
        let ok = DecomposeResult::Ok {
            children_count: 3,
            summary: "decomposed into 3 children".to_string(),
        };
        let json = serde_json::to_string(&ok).expect("serialize Ok");
        let parsed: DecomposeResult = serde_json::from_str(&json).expect("deserialize Ok");
        assert_eq!(parsed, ok, "D-13: DecomposeResult::Ok must round-trip");

        let nw = DecomposeResult::NotWired {
            message: "Use: hermes kanban decompose t_abc".to_string(),
        };
        let json = serde_json::to_string(&nw).expect("serialize NotWired");
        let parsed: DecomposeResult =
            serde_json::from_str(&json).expect("deserialize NotWired");
        assert_eq!(parsed, nw, "D-13: DecomposeResult::NotWired must round-trip");
    }
}

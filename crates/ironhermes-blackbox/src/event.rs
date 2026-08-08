use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Phase 39.2: 8-stage taxonomy for black-box recording.
/// All variants are serialized as snake_case JSON strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Input,
    Routing,
    Retrieval,
    Tool,
    Model,
    Guardrail,
    Output,
    Error,
}

/// Phase 39.2: one recorded event in the black-box JSONL file.
///
/// Appended by `BlackBoxRecorder::try_record`. Each field maps to the
/// Python prototype `RecordedEvent` shape in `docs/ai_tracking.py`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlackBoxEvent {
    /// Per-turn run_id — correlates all events for one agent turn.
    /// Populated from `TurnRequest::turn_id` when available, otherwise
    /// generated fresh by `AgentRuntime::run_turn`.
    pub run_id: Uuid,
    /// Which pipeline stage this event belongs to.
    pub stage: Stage,
    /// Semantic event name, e.g. "turn_started", "tool_dispatched".
    pub event_name: String,
    /// Unix epoch milliseconds at the moment the event was emitted.
    pub timestamp_ms: u64,
    /// Schema version for replay tooling — e.g. "hermes-2026-06".
    pub workflow_version: String,
    /// Stage-specific payload. PII is redacted by the writer task before
    /// the line is flushed to disk.
    pub metadata: serde_json::Value,
}

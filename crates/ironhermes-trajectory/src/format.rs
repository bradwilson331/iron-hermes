//! Wire format spec for trajectory JSONL entries (Phase 25.3 D-T-1).
//!
//! Each `trajectories.jsonl` line is one TrajectoryEntry serialized as JSON.
//! Format is IronHermes-original (NOT a port of Python `agent/trajectory.py`,
//! which is session-level ShareGPT — see RESEARCH.md "CRITICAL FINDING").
//!
//! D-T-1 fields: { name, args (post-redact), result_or_error, duration_ms,
//! impact_level (Read=0/Write=5/SystemChange=10), turn_index, tool_call_id, ts }
//!
//! Implementation note: `result_or_error` is flattened to `result: Option<String>`
//! + `error: Option<String>` (mutually exclusive) so the wire shape derives via
//! standard serde — no custom impl. The golden-file test in tests/format.rs locks
//! the on-disk shape; future changes must intentionally update both that test and
//! any downstream consumers (Phase 25.4 Curator, RL pipelines).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Impact level for a tool call (D-T-1).
///
/// Discriminant values are wire-stable: Read=0, Write=5, SystemChange=10.
/// Phase 25.4 Curator's heuristic gate (D-C-2) reads these as numeric weights.
/// Serializes as snake_case strings so the JSONL output is human-readable;
/// downstream consumers map back to the discriminant via the enum if needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ImpactLevel {
    Read = 0,
    Write = 5,
    SystemChange = 10,
}

impl ImpactLevel {
    /// Numeric weight for Curator heuristic scoring (Phase 25.4 D-C-2).
    pub fn weight(self) -> u8 {
        self as u8
    }
}

/// Per-tool-call trajectory record (Phase 25.3 D-T-1, IronHermes-original format).
///
/// Written one-per-line to `trajectories.jsonl` by `TrajectoryWriter::append()`
/// (Plan 4). Never read back during the same session — append-only.
/// Plan 9 (AgentLoop callback) constructs one entry per `execute_tool_call`.
///
/// `result` and `error` are mutually exclusive: success populates `result` and
/// leaves `error` as None; failure populates `error` and leaves `result` as None.
/// Together they encode the D-T-1 `result_or_error` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryEntry {
    /// Tool name (e.g., "write_file", "terminal", "web_extract").
    pub name: String,
    /// Tool args after redaction via `Tool::redact_args()` (Plan 5 / Discretion D-2).
    pub args: Value,
    /// Success result text — Some(...) on success, None on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Failure error text — Some(...) on failure, None on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Wall-clock duration of `tool.execute(args)` in milliseconds.
    pub duration_ms: u64,
    /// Read=0, Write=5, SystemChange=10 (D-T-1).
    pub impact_level: ImpactLevel,
    /// 0-indexed turn within the session (number of complete user-assistant exchanges).
    pub turn_index: usize,
    /// Tool call id from the LLM response (e.g., "toolu_abc123" — provider-specific).
    pub tool_call_id: String,
    /// ISO 8601 UTC timestamp (chrono::Utc::now().to_rfc3339()).
    pub ts: String,
}

impl TrajectoryEntry {
    /// Construct a success entry — `result_or_error` populated as Ok-shape.
    #[allow(clippy::too_many_arguments)]
    pub fn success(
        name: impl Into<String>,
        args: Value,
        result: impl Into<String>,
        duration_ms: u64,
        impact_level: ImpactLevel,
        turn_index: usize,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            args,
            result: Some(result.into()),
            error: None,
            duration_ms,
            impact_level,
            turn_index,
            tool_call_id: tool_call_id.into(),
            ts: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Construct a failure entry — `result_or_error` populated as Err-shape.
    #[allow(clippy::too_many_arguments)]
    pub fn failure(
        name: impl Into<String>,
        args: Value,
        error: impl Into<String>,
        duration_ms: u64,
        impact_level: ImpactLevel,
        turn_index: usize,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            args,
            result: None,
            error: Some(error.into()),
            duration_ms,
            impact_level,
            turn_index,
            tool_call_id: tool_call_id.into(),
            ts: chrono::Utc::now().to_rfc3339(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impact_level_discriminants_are_0_5_10() {
        assert_eq!(ImpactLevel::Read.weight(), 0);
        assert_eq!(ImpactLevel::Write.weight(), 5);
        assert_eq!(ImpactLevel::SystemChange.weight(), 10);
    }

    #[test]
    fn impact_level_serializes_as_snake_case() {
        assert_eq!(serde_json::to_string(&ImpactLevel::Read).unwrap(), "\"read\"");
        assert_eq!(serde_json::to_string(&ImpactLevel::Write).unwrap(), "\"write\"");
        assert_eq!(
            serde_json::to_string(&ImpactLevel::SystemChange).unwrap(),
            "\"system_change\""
        );
    }

    #[test]
    fn success_entry_omits_error_field() {
        let e = TrajectoryEntry::success(
            "write_file",
            serde_json::json!({"path": "/tmp/x"}),
            "wrote 0 bytes",
            10,
            ImpactLevel::Write,
            0,
            "toolu_abc",
        );
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"result\":\"wrote 0 bytes\""));
        assert!(
            !json.contains("\"error\""),
            "success entry must skip error field; got {json}"
        );
    }

    #[test]
    fn failure_entry_omits_result_field() {
        let e = TrajectoryEntry::failure(
            "terminal",
            serde_json::json!({"cmd": "false"}),
            "exit 1",
            5,
            ImpactLevel::SystemChange,
            2,
            "toolu_xyz",
        );
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"error\":\"exit 1\""));
        assert!(
            !json.contains("\"result\""),
            "failure entry must skip result field; got {json}"
        );
    }

    #[test]
    fn entry_roundtrips() {
        let e = TrajectoryEntry::success(
            "read_file",
            serde_json::json!({"path": "Cargo.toml"}),
            "1234 bytes",
            3,
            ImpactLevel::Read,
            1,
            "toolu_rt",
        );
        let json = serde_json::to_string(&e).unwrap();
        let parsed: TrajectoryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "read_file");
        assert_eq!(parsed.duration_ms, 3);
        assert_eq!(parsed.impact_level, ImpactLevel::Read);
        assert_eq!(parsed.turn_index, 1);
        assert_eq!(parsed.tool_call_id, "toolu_rt");
        assert_eq!(parsed.result.as_deref(), Some("1234 bytes"));
        assert!(parsed.error.is_none());
    }
}

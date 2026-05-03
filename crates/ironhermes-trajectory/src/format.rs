//! Wire format spec for trajectory JSONL entries (Phase 25.3 D-T-1).
//!
//! Stub — implementation lands in Task 2 of the same plan.
//! Re-exports in `lib.rs` are pre-declared so the workspace build is green
//! once the crate is added to `workspace.members`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Impact level for a tool call (D-T-1) — placeholder, populated in Task 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ImpactLevel {
    Read = 0,
    Write = 5,
    SystemChange = 10,
}

/// Per-tool-call trajectory record (Phase 25.3 D-T-1) — placeholder, populated in Task 2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryEntry {
    pub name: String,
    pub args: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub duration_ms: u64,
    pub impact_level: ImpactLevel,
    pub turn_index: usize,
    pub tool_call_id: String,
    pub ts: String,
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

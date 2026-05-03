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

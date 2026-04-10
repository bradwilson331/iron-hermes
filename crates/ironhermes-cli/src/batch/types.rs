use serde::{Deserialize, Serialize};

/// Input entry from JSONL file.
#[derive(Debug, Clone, Deserialize)]
pub struct BatchEntry {
    pub prompt: String,
    /// Optional system prompt override.
    #[serde(default)]
    pub system: Option<String>,
    /// Optional tool allowlist.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
}

/// A single ShareGPT conversation turn (D-07, D-08).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareGptTurn {
    pub from: String,
    pub value: String,
}

/// Quality assessment metadata (D-13).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityResult {
    pub passed: bool,
    pub reasons: Vec<String>,
}

/// Token usage metadata for trajectory (D-09).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageInfo {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
}

/// Full trajectory output line in ShareGPT format (D-09, D-10).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryLine {
    pub id: String,
    pub model: String,
    pub timestamp: String,
    pub usage: UsageInfo,
    pub turns: usize,
    pub quality: QualityResult,
    pub conversations: Vec<ShareGptTurn>,
    /// Only present on rejected trajectories (D-11).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
}

/// Checkpoint entry for a completed prompt (D-05, D-06).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointEntry {
    pub status: String,
    pub timestamp: String,
}

/// Persistent batch run record for `batch list` (D-04).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchRunRecord {
    pub id: String,
    pub input_file: String,
    pub output_file: String,
    pub total_entries: usize,
    pub completed: usize,
    pub passed: usize,
    pub rejected: usize,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: String,
}

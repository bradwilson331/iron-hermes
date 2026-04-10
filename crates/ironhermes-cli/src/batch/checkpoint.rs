use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use super::types::CheckpointEntry;

/// SHA-256 hash of the prompt string, returned as 64-char lowercase hex (D-05).
pub fn prompt_hash(_prompt: &str) -> String {
    todo!("Implemented in Task 2")
}

/// Load checkpoint from JSON file. Returns empty map if file doesn't exist (D-06).
pub fn load_checkpoint(_path: &Path) -> Result<HashMap<String, CheckpointEntry>> {
    todo!("Implemented in Task 2")
}

/// Atomically save checkpoint to JSON file (temp + rename to prevent corruption).
pub fn save_checkpoint(_path: &Path, _data: &HashMap<String, CheckpointEntry>) -> Result<()> {
    todo!("Implemented in Task 2")
}

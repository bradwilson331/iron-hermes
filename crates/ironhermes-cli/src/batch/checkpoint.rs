use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use super::types::CheckpointEntry;

/// SHA-256 hash of the prompt string, returned as 64-char lowercase hex (D-05).
pub fn prompt_hash(prompt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Load checkpoint from JSON file. Returns empty map if file doesn't exist (D-06).
pub fn load_checkpoint(path: &Path) -> Result<HashMap<String, CheckpointEntry>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let data = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read checkpoint: {}", path.display()))?;
    if data.trim().is_empty() {
        return Ok(HashMap::new());
    }
    serde_json::from_str(&data)
        .with_context(|| format!("Failed to parse checkpoint: {}", path.display()))
}

/// Atomically save checkpoint to JSON file (temp + rename to prevent corruption).
pub fn save_checkpoint(path: &Path, data: &HashMap<String, CheckpointEntry>) -> Result<()> {
    let tmp = path.with_extension("checkpoint.tmp");
    let json = serde_json::to_string_pretty(data)?;
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

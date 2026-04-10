use anyhow::Result;
use std::path::PathBuf;

/// Run batch prompt execution from JSONL input (D-01).
pub async fn cmd_run(
    _input: PathBuf,
    _output: Option<PathBuf>,
    _workers: Option<usize>,
    _model: Option<String>,
) -> Result<()> {
    todo!("Implemented in Task 2")
}

/// Show progress of current/last batch run (D-02).
pub async fn cmd_status() -> Result<()> {
    todo!("Implemented in Task 2")
}

/// Gracefully cancel the running batch (D-03).
pub async fn cmd_cancel() -> Result<()> {
    todo!("Implemented in Task 2")
}

/// List past batch runs with summary (D-04).
pub async fn cmd_list() -> Result<()> {
    todo!("Implemented in Task 2")
}

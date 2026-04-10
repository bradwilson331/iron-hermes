pub mod types;
pub mod runner;
pub mod checkpoint;
pub mod sharegpt;
#[cfg(test)]
mod tests;

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

/// Batch processing subcommands (D-01).
#[derive(Subcommand)]
pub enum BatchCommands {
    /// Run batch prompt execution from JSONL input
    Run {
        /// Path to input JSONL file (one {"prompt": "..."} per line)
        input: PathBuf,

        /// Output JSONL file path (default: batch_output/<input_stem>_output.jsonl)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Number of parallel workers (default: from config, fallback 4)
        #[arg(short, long)]
        workers: Option<usize>,

        /// Model override (default: from config)
        #[arg(short, long)]
        model: Option<String>,
    },
    /// Show progress of current/last batch run (D-02)
    Status,
    /// Gracefully cancel the running batch (D-03)
    Cancel,
    /// List past batch runs with summary (D-04)
    List,
}

pub async fn handle_batch_command(cmd: BatchCommands) -> Result<()> {
    match cmd {
        BatchCommands::Run { input, output, workers, model } => {
            runner::cmd_run(input, output, workers, model).await
        }
        BatchCommands::Status => {
            runner::cmd_status().await
        }
        BatchCommands::Cancel => {
            runner::cmd_cancel().await
        }
        BatchCommands::List => {
            runner::cmd_list().await
        }
    }
}

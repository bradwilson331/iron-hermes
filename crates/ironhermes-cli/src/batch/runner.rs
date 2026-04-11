use anyhow::{Context, Result};
use chrono::Utc;
use colored::Colorize;
use ironhermes_agent::{AgentLoop, build_main_client};
use ironhermes_core::{ChatMessage, Config, ProviderResolver};
use ironhermes_tools::ToolRegistry;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;
use uuid::Uuid;

use super::checkpoint::{load_checkpoint, prompt_hash, save_checkpoint};
use super::filters;
use super::sharegpt::messages_to_sharegpt;
use super::types::{BatchEntry, BatchRunRecord, CheckpointEntry, TrajectoryLine, UsageInfo};

/// Run batch prompt execution from JSONL input (D-01).
pub async fn cmd_run(
    input: PathBuf,
    output: Option<PathBuf>,
    workers: Option<usize>,
    model: Option<String>,
) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let start_time = std::time::Instant::now();

    // Resolve output path
    let output_path = match output {
        Some(p) => p,
        None => {
            let stem = input
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("batch");
            let output_dir = PathBuf::from(&config.batch.output_dir);
            std::fs::create_dir_all(&output_dir)
                .with_context(|| format!("Failed to create output dir: {}", output_dir.display()))?;
            output_dir.join(format!("{}_output.jsonl", stem))
        }
    };

    let reject_path = {
        let stem = output_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        output_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join(format!("{}_rejected.jsonl", stem.trim_end_matches("_output")))
    };

    // Load checkpoint
    let checkpoint_path = output_path.with_extension("checkpoint.json");
    let checkpoint = load_checkpoint(&checkpoint_path)
        .context("Failed to load checkpoint")?;

    // Resolve worker count
    let worker_count = workers.unwrap_or(config.batch.workers).max(1);
    let max_turns = config.batch.max_turns;

    // Resolve model and build provider resolver
    let model_name = model.unwrap_or_else(|| config.model.default.clone());
    let resolver = ProviderResolver::build(&config)
        .context("Failed to build provider resolver")?;

    // Read all entries from input JSONL
    let input_file = tokio::fs::File::open(&input)
        .await
        .with_context(|| format!("Failed to open input file: {}", input.display()))?;
    let reader = BufReader::new(input_file);
    let mut lines = reader.lines();

    let mut all_entries: Vec<(String, BatchEntry)> = Vec::new();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let entry: BatchEntry = serde_json::from_str(&line)
            .with_context(|| format!("Failed to parse JSONL line: {}", line))?;
        let hash = prompt_hash(&entry.prompt);
        all_entries.push((hash, entry));
    }

    let total_entries = all_entries.len();
    let pending: Vec<(String, BatchEntry)> = all_entries
        .into_iter()
        .filter(|(hash, _)| !checkpoint.contains_key(hash))
        .collect();

    let skipped = total_entries - pending.len();
    if skipped > 0 {
        println!(
            "{} Skipping {} already-completed entries (checkpoint resume)",
            "Info:".cyan(),
            skipped
        );
    }

    println!(
        "{} Starting batch: {} entries, {} workers, model={}",
        "Batch:".bold().cyan(),
        pending.len(),
        worker_count,
        model_name
    );

    // Cancel sentinel path
    let cancel_path = ironhermes_core::get_hermes_home()
        .join("batch")
        .join("cancel");

    // Timestamp-guarded stale sentinel removal (Plan 04 fix 1):
    // Only remove the sentinel if it predates process start — a fresh sentinel means
    // cmd_cancel was issued concurrently and must be honored.
    let run_start = std::time::SystemTime::now();
    clean_stale_sentinel(&cancel_path, run_start);

    // Run record setup
    let run_id = Uuid::new_v4().to_string();
    let run_record = BatchRunRecord {
        id: run_id.clone(),
        input_file: input.display().to_string(),
        output_file: output_path.display().to_string(),
        total_entries,
        completed: 0,
        passed: 0,
        rejected: 0,
        started_at: Utc::now().to_rfc3339(),
        finished_at: None,
        status: "running".to_string(),
    };
    save_run_record(&run_record).await?;

    // mpsc channel: (TrajectoryLine, prompt_hash, status)
    let (tx, mut rx) = mpsc::channel::<(TrajectoryLine, String)>(256);

    // Writer task
    let checkpoint_path_clone = checkpoint_path.clone();
    let output_path_clone = output_path.clone();
    let reject_path_clone = reject_path.clone();
    let _run_id_clone = run_id.clone();

    let writer_handle = tokio::spawn(async move {
        let mut checkpoint_data: HashMap<String, CheckpointEntry> = load_checkpoint(&checkpoint_path_clone)
            .unwrap_or_default();
        let mut passed_count = 0usize;
        let mut rejected_count = 0usize;

        while let Some((trajectory, hash)) = rx.recv().await {
            let line = serde_json::to_string(&trajectory).unwrap_or_default() + "\n";

            if trajectory.quality.passed {
                // Append to output file
                if let Ok(mut file) = tokio::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&output_path_clone)
                    .await
                {
                    let _ = file.write_all(line.as_bytes()).await;
                }
                passed_count += 1;
            } else {
                // Append to reject file (D-11)
                if let Ok(mut file) = tokio::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&reject_path_clone)
                    .await
                {
                    let _ = file.write_all(line.as_bytes()).await;
                }
                rejected_count += 1;
            }

            // Update checkpoint
            checkpoint_data.insert(
                hash,
                CheckpointEntry {
                    status: if trajectory.quality.passed {
                        "completed".to_string()
                    } else {
                        "rejected".to_string()
                    },
                    timestamp: Utc::now().to_rfc3339(),
                },
            );
            let _ = save_checkpoint(&checkpoint_path_clone, &checkpoint_data);
        }

        (passed_count, rejected_count)
    });

    let semaphore = Arc::new(Semaphore::new(worker_count));
    let mut join_set: JoinSet<()> = JoinSet::new();
    let mut cancelled = false;

    'dispatch: for (hash, entry) in pending {
        // Check cancel sentinel before attempting semaphore acquire (D-03)
        if cancel_path.exists() {
            println!(
                "{} Cancel sentinel detected — stopping dispatch",
                "Batch:".yellow()
            );
            cancelled = true;
            break 'dispatch;
        }

        // Plan 04 fix 2: select!-based cancel polling during semaphore acquire.
        // If all worker slots are busy, the acquire would block indefinitely and
        // the cancel sentinel check above would never run. Poll every 500ms so
        // cancel is detected within 500ms even under full semaphore contention.
        let permit = loop {
            tokio::select! {
                result = semaphore.clone().acquire_owned() => {
                    break result?;
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                    if cancel_path.exists() {
                        println!(
                            "{} Cancel sentinel detected — stopping dispatch",
                            "Batch:".yellow()
                        );
                        cancelled = true;
                        break 'dispatch;
                    }
                }
            }
        };
        let tx = tx.clone();
        let client = build_main_client(&resolver)?;
        let registry = Arc::new({
            let mut r = ToolRegistry::new();
            r.register_defaults();
            r
        });
        let hash_clone = hash.clone();
        let model_for_traj = model_name.clone();

        join_set.spawn(async move {
            let _permit = permit; // dropped when task ends

            let mut messages = Vec::new();
            if let Some(system) = &entry.system {
                messages.push(ChatMessage::system(system));
            }
            messages.push(ChatMessage::user(&entry.prompt));

            let mut agent = AgentLoop::new(client, registry.clone(), max_turns);
            match agent.run(messages).await {
                Ok(result) => {
                    // Run quality filters (D-12, D-13)
                    let quality = filters::run_filters(&result, &registry);
                    let rejection_reason = if quality.passed {
                        None
                    } else {
                        Some(quality.reasons.join("; "))
                    };
                    let conversations = messages_to_sharegpt(&result.messages);
                    let trajectory = TrajectoryLine {
                        id: hash_clone.clone(),
                        model: model_for_traj,
                        timestamp: Utc::now().to_rfc3339(),
                        usage: UsageInfo {
                            prompt_tokens: result.total_usage.prompt_tokens,
                            completion_tokens: result.total_usage.completion_tokens,
                        },
                        turns: result.turns_used,
                        quality,
                        conversations,
                        rejection_reason,
                    };
                    let _ = tx.send((trajectory, hash_clone)).await;
                }
                Err(e) => {
                    eprintln!("{} Agent error for prompt hash {}: {}", "Error:".red(), &hash_clone[..8], e);
                }
            }
        });
    }

    // Drop sender so writer task knows when all workers are done
    drop(tx);

    // Wait for all workers
    while let Some(res) = join_set.join_next().await {
        if let Err(e) = res {
            eprintln!("{} Worker task panicked: {}", "Error:".red(), e);
        }
    }

    // Wait for writer and get counts
    let (passed_count, rejected_count) = writer_handle.await.unwrap_or((0, 0));
    let elapsed = start_time.elapsed();

    // Update run record
    let final_status = if cancelled { "cancelled" } else { "completed" };
    let mut final_record = BatchRunRecord {
        id: run_id,
        input_file: input.display().to_string(),
        output_file: output_path.display().to_string(),
        total_entries,
        completed: passed_count + rejected_count,
        passed: passed_count,
        rejected: rejected_count,
        started_at: Utc::now().to_rfc3339(), // approximate
        finished_at: Some(Utc::now().to_rfc3339()),
        status: final_status.to_string(),
    };
    // Preserve started_at from original record
    if let Ok(records) = load_run_records().await {
        if let Some(orig) = records.iter().find(|r| r.id == final_record.id) {
            final_record.started_at = orig.started_at.clone();
        }
    }
    save_run_record(&final_record).await?;

    // Delete checkpoint on successful completion (D-06)
    if !cancelled && checkpoint_path.exists() {
        let _ = std::fs::remove_file(&checkpoint_path);
    }

    println!(
        "\n{} {} total={} passed={} rejected={} elapsed={:.1}s",
        "Done:".bold().green(),
        final_status,
        total_entries,
        passed_count,
        rejected_count,
        elapsed.as_secs_f64()
    );

    Ok(())
}

/// Show progress of current/last batch run (D-02).
pub async fn cmd_status() -> Result<()> {
    let records = load_run_records().await?;
    if records.is_empty() {
        println!("{}", "No batch runs found.".dimmed());
        return Ok(());
    }

    // Find latest running or last completed
    let record = records
        .iter()
        .find(|r| r.status == "running")
        .or_else(|| records.last())
        .unwrap();

    println!("{}", "Batch Status".bold().cyan());
    println!("{}", "─".repeat(40));
    println!("  Run ID:   {}", &record.id[..8]);
    println!("  Input:    {}", record.input_file);
    println!("  Output:   {}", record.output_file);
    println!("  Status:   {}", record.status);
    println!("  Total:    {}", record.total_entries);
    println!("  Done:     {}", record.completed);
    println!("  Passed:   {}", record.passed);
    println!("  Rejected: {}", record.rejected);
    println!("  Started:  {}", record.started_at);
    if let Some(ref finished) = record.finished_at {
        println!("  Finished: {}", finished);
    }

    Ok(())
}

/// Gracefully cancel the running batch (D-03).
pub async fn cmd_cancel() -> Result<()> {
    let cancel_dir = ironhermes_core::get_hermes_home().join("batch");
    std::fs::create_dir_all(&cancel_dir)?;
    let cancel_path = cancel_dir.join("cancel");
    std::fs::write(&cancel_path, "")?;
    println!(
        "{} Cancel sentinel written. Running batch will stop after current workers finish.",
        "Batch:".yellow()
    );
    Ok(())
}

/// List past batch runs with summary (D-04).
pub async fn cmd_list() -> Result<()> {
    let records = load_run_records().await?;
    if records.is_empty() {
        println!("{}", "No batch runs found.".dimmed());
        return Ok(());
    }

    println!(
        "{:<10} {:<30} {:<8} {:<8} {:<8} {:<12}",
        "ID".bold(),
        "Input".bold(),
        "Total".bold(),
        "Passed".bold(),
        "Reject".bold(),
        "Status".bold(),
    );
    println!("{}", "─".repeat(80));

    for record in &records {
        let id_short = &record.id[..8.min(record.id.len())];
        let input_short = if record.input_file.len() > 28 {
            format!("...{}", &record.input_file[record.input_file.len()-25..])
        } else {
            record.input_file.clone()
        };
        let status_colored = match record.status.as_str() {
            "completed" => record.status.green(),
            "cancelled" => record.status.yellow(),
            "running" => record.status.cyan(),
            _ => record.status.red(),
        };
        println!(
            "{:<10} {:<30} {:<8} {:<8} {:<8} {}",
            id_short,
            input_short,
            record.total_entries,
            record.passed,
            record.rejected,
            status_colored,
        );
    }

    Ok(())
}

/// Timestamp-guarded cancel sentinel cleanup (Plan 04 fix 1).
///
/// Removes the sentinel file only if its mtime predates `run_start`.
/// A sentinel newer than `run_start` was created by a concurrent `cmd_cancel` and must be honored.
/// If mtime is unreadable (some platforms) the file is removed to avoid stuck state.
/// Extracted as a public fn so unit tests can call it directly without spawning a full run.
pub fn clean_stale_sentinel(cancel_path: &std::path::Path, run_start: std::time::SystemTime) {
    if cancel_path.exists() {
        if let Ok(meta) = std::fs::metadata(cancel_path) {
            if let Ok(mtime) = meta.modified() {
                if mtime < run_start {
                    // Stale sentinel from a previous run — safe to remove
                    let _ = std::fs::remove_file(cancel_path);
                }
                // else: fresh sentinel (created after/at process start) — honor it, leave in place
            } else {
                // Cannot read mtime (some platforms) — remove to avoid stuck state
                let _ = std::fs::remove_file(cancel_path);
            }
        }
    }
}

/// Derive reject file path from output path: foo.jsonl -> foo_rejected.jsonl (D-11).
pub fn reject_file_path(output: &std::path::Path) -> std::path::PathBuf {
    let stem = output.file_stem().unwrap_or_default().to_string_lossy();
    let parent = output.parent().unwrap_or(std::path::Path::new("."));
    parent.join(format!("{}_rejected.jsonl", stem))
}

// =============================================================================
// Run record persistence helpers
// =============================================================================

fn runs_file_path() -> PathBuf {
    ironhermes_core::get_hermes_home().join("batch").join("runs.json")
}

async fn load_run_records() -> Result<Vec<BatchRunRecord>> {
    let path = runs_file_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = tokio::fs::read_to_string(&path).await?;
    if data.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&data).context("Failed to parse runs.json")
}

async fn save_run_record(record: &BatchRunRecord) -> Result<()> {
    let path = runs_file_path();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut records = load_run_records().await.unwrap_or_default();

    // Update existing or append
    if let Some(existing) = records.iter_mut().find(|r| r.id == record.id) {
        *existing = record.clone();
    } else {
        records.push(record.clone());
    }

    let json = serde_json::to_string_pretty(&records)?;
    let tmp = path.with_extension("runs.tmp");
    tokio::fs::write(&tmp, &json).await?;
    tokio::fs::rename(&tmp, &path).await?;

    Ok(())
}

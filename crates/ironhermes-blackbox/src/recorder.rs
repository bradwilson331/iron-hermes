use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::event::{BlackBoxEvent, Stage};
use crate::redact::redact;

/// Phase 39.2: append-only black-box recorder for AI agent turns.
///
/// Writes one JSONL line per event to `~/.ironhermes/logs/blackbox-YYYY-MM-DD.jsonl`.
/// All I/O is off the hot path — events are sent through a bounded mpsc channel
/// and flushed by a background writer task. `try_record` is fire-and-forget:
/// a full channel drops the event with a `tracing::warn!` rather than blocking.
pub struct BlackBoxRecorder {
    sender: tokio::sync::mpsc::Sender<BlackBoxEvent>,
}

impl BlackBoxRecorder {
    /// Spawn the background writer task and return a recorder that sends to it.
    ///
    /// `workflow_version` — schema version string embedded in every event
    /// (e.g. `"hermes-2026-06"`). `home_dir` — the IronHermes home directory
    /// (from `ironhermes_core::get_hermes_home()`); JSONL files are written to
    /// `home_dir/logs`.
    pub fn new(workflow_version: String, home_dir: PathBuf) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel::<BlackBoxEvent>(256);
        tokio::spawn(writer_task(rx, home_dir, workflow_version));
        Self { sender: tx }
    }

    /// Fire-and-forget event emit. Drops the event with a warning if the
    /// channel is full (backpressure guard — never blocks the calling task).
    pub fn try_record(&self, event: BlackBoxEvent) {
        if self.sender.try_send(event).is_err() {
            tracing::warn!(
                "ironhermes-blackbox: channel full — black-box event dropped \
                 (increase channel buffer or reduce event rate)"
            );
        }
    }

    /// Construct a `BlackBoxEvent` with the current wall-clock timestamp.
    ///
    /// Callers supply `run_id`, `stage`, `event_name`, and `metadata`; this
    /// function fills `timestamp_ms` and `workflow_version` automatically.
    pub fn make_event(
        run_id: Uuid,
        stage: Stage,
        event_name: impl Into<String>,
        workflow_version: &str,
        metadata: serde_json::Value,
    ) -> BlackBoxEvent {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        BlackBoxEvent {
            run_id,
            stage,
            event_name: event_name.into(),
            timestamp_ms,
            workflow_version: workflow_version.to_string(),
            metadata,
        }
    }
}

/// Background writer task: receives events, redacts PII from metadata, and
/// appends them as JSONL lines to a daily-rotating file in `home_dir/logs`.
///
/// Rotation happens lazily on the first write of a new calendar day — the
/// previous file is closed and a fresh one opened. Each event is written as a
/// single `write_all` call to preserve JSONL atomicity on typical filesystems.
async fn writer_task(
    mut rx: tokio::sync::mpsc::Receiver<BlackBoxEvent>,
    home_dir: PathBuf,
    _workflow_version: String,
) {
    let log_dir = home_dir.join("logs");
    let mut current_date = String::new();
    let mut writer: Option<tokio::io::BufWriter<tokio::fs::File>> = None;

    while let Some(mut event) = rx.recv().await {
        // Compute today's date string for rotation check.
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();

        if today != current_date {
            // Close previous file (flush happens on drop).
            if let Some(mut w) = writer.take() {
                let _ = w.flush().await;
            }

            // Open new daily file in append mode.
            if let Err(e) = tokio::fs::create_dir_all(&log_dir).await {
                tracing::warn!(
                    error = %e,
                    path = %log_dir.display(),
                    "ironhermes-blackbox: failed to create log directory; event dropped"
                );
                continue;
            }

            let path = log_dir.join(format!("blackbox-{today}.jsonl"));
            match tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
            {
                Ok(file) => {
                    current_date = today;
                    writer = Some(tokio::io::BufWriter::new(file));
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        path = %path.display(),
                        "ironhermes-blackbox: failed to open JSONL file; event dropped"
                    );
                    continue;
                }
            }
        }

        // Redact PII from metadata before writing.
        event.metadata = redact(event.metadata);

        let Some(ref mut w) = writer else { continue };

        match serde_json::to_string(&event) {
            Ok(line) => {
                let mut bytes = line.into_bytes();
                bytes.push(b'\n');
                if let Err(e) = w.write_all(&bytes).await {
                    tracing::warn!(
                        error = %e,
                        "ironhermes-blackbox: write failed; event dropped"
                    );
                } else {
                    // Flush after each event so partial-file reads see complete lines.
                    if let Err(e) = w.flush().await {
                        tracing::warn!(
                            error = %e,
                            "ironhermes-blackbox: flush failed"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "ironhermes-blackbox: failed to serialize event; event dropped"
                );
            }
        }
    }
}

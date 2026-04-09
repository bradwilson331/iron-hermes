//! Disk-persistent retry queue for failed webhook deliveries.
//!
//! File location: `{hermes_home}/hooks/retry-queue.jsonl`
//! Each line is a JSON-serialized `RetryEntry`.
//!
//! On startup, `drain()` reads all entries, discards those older than
//! `queue_ttl_hours`, and returns the rest for re-delivery.
//! After drain, the file is truncated (entries are either re-delivered
//! or re-enqueued on failure).

use std::io::{BufRead, Write};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::event::HookEvent;

/// A single entry in the persistent retry queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryEntry {
    /// The webhook endpoint URL this delivery targets.
    pub endpoint_url: String,
    /// The serialized HookEvent to deliver.
    pub event: HookEvent,
    /// When this entry was first queued (for TTL enforcement).
    pub queued_at: DateTime<Utc>,
    /// How many delivery attempts have been made so far.
    pub attempts: u32,
}

/// Disk-persistent retry queue backed by a JSONL file.
///
/// File location: `{hermes_home}/hooks/retry-queue.jsonl`
/// Each line is a JSON-serialized RetryEntry.
///
/// On startup, `drain()` reads all entries, discards those older than
/// `queue_ttl_hours`, and returns the rest for re-delivery.
/// After drain, the file is truncated (entries are either re-delivered
/// or re-enqueued on failure).
pub struct RetryQueue {
    path: PathBuf,
}

impl RetryQueue {
    /// Create a new RetryQueue. Creates parent directories if needed.
    pub fn new(path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(Self { path })
    }

    /// Default path using get_hermes_home().
    pub fn default_path() -> PathBuf {
        let home = ironhermes_core::constants::get_hermes_home();
        home.join("hooks").join("retry-queue.jsonl")
    }

    /// Append a failed delivery to the persistent queue file.
    /// Uses append mode + newline-terminated JSON (atomic enough for single-writer).
    pub fn enqueue(&self, entry: &RetryEntry) -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let json = serde_json::to_string(entry)?;
        writeln!(file, "{}", json)?;
        Ok(())
    }

    /// Read all entries from disk, discard entries older than `ttl_hours`,
    /// truncate the file, and return valid entries for re-delivery.
    /// This is called once at startup.
    pub fn drain(&self, ttl_hours: u32) -> Vec<RetryEntry> {
        let file = match std::fs::File::open(&self.path) {
            Ok(f) => f,
            Err(_) => return Vec::new(), // No queue file = nothing to drain
        };

        let now = Utc::now();
        let ttl = chrono::Duration::hours(ttl_hours as i64);
        let reader = std::io::BufReader::new(file);

        let mut valid_entries = Vec::new();
        for line in reader.lines() {
            let line = match line {
                Ok(l) if !l.trim().is_empty() => l,
                _ => continue,
            };
            match serde_json::from_str::<RetryEntry>(&line) {
                Ok(entry) => {
                    if now.signed_duration_since(entry.queued_at) < ttl {
                        valid_entries.push(entry);
                    } else {
                        tracing::debug!(
                            url = %entry.endpoint_url,
                            queued_at = %entry.queued_at,
                            "Discarding expired retry queue entry (older than {} hours)",
                            ttl_hours
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("Skipping malformed retry queue entry: {}", e);
                }
            }
        }

        // Truncate the file after draining — entries will be re-enqueued if
        // re-delivery fails, or simply gone if delivery succeeds.
        if let Err(e) = std::fs::write(&self.path, "") {
            tracing::warn!("Failed to truncate retry queue file: {}", e);
        }

        valid_entries
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::HookEventKind;

    fn make_entry(url: &str, queued_at: DateTime<Utc>) -> RetryEntry {
        RetryEntry {
            endpoint_url: url.to_string(),
            event: HookEvent::new(
                "req-test",
                HookEventKind::MessageReceived {
                    platform: "telegram".to_string(),
                    chat_id: "42".to_string(),
                    content_preview: "hello".to_string(),
                },
            ),
            queued_at,
            attempts: 1,
        }
    }

    #[test]
    fn test_enqueue_and_drain() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("retry-queue.jsonl");
        let queue = RetryQueue::new(path).expect("new");

        let now = Utc::now();
        queue.enqueue(&make_entry("https://example.com/a", now)).expect("enqueue 1");
        queue.enqueue(&make_entry("https://example.com/b", now)).expect("enqueue 2");

        let entries = queue.drain(24);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_drain_discards_expired() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("retry-queue.jsonl");
        let queue = RetryQueue::new(path).expect("new");

        // Entry queued 48 hours ago — should be discarded with 24h TTL
        let old_time = Utc::now() - chrono::Duration::hours(48);
        queue.enqueue(&make_entry("https://example.com/old", old_time)).expect("enqueue");

        let entries = queue.drain(24);
        assert!(entries.is_empty(), "expected expired entry to be discarded");
    }

    #[test]
    fn test_drain_missing_file_returns_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("nonexistent-retry-queue.jsonl");
        let queue = RetryQueue::new(path).expect("new");

        let entries = queue.drain(24);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_drain_truncates_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("retry-queue.jsonl");
        let queue = RetryQueue::new(path.clone()).expect("new");

        let now = Utc::now();
        queue.enqueue(&make_entry("https://example.com/x", now)).expect("enqueue");

        // File should have content before drain
        let content_before = std::fs::read_to_string(&path).expect("read before");
        assert!(!content_before.trim().is_empty(), "file should have content before drain");

        queue.drain(24);

        // File should be empty after drain
        let content_after = std::fs::read_to_string(&path).expect("read after");
        assert!(content_after.is_empty(), "file should be empty after drain");
    }
}

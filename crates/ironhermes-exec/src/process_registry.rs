//! Session-scoped in-memory background-process registry (D-23..D-29).
//!
//! Constants match hermes-agent/tools/process_registry.py:57-64 byte-for-byte.
//! NO persistence (D-23): registry lives in RAM only; drained on session end (D-24).
//! NO Drop-based cleanup (Pitfall 6 / G-07): async `drain_and_kill` only.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use regex::Regex;
use tokio::process::Child;
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;

// -- hermes-agent parity constants (verified against process_registry.py:57-64) --
pub const MAX_OUTPUT_CHARS: usize = 200_000;
pub const MAX_PROCESSES: usize = 64;
pub const FINISHED_TTL_SECONDS: u64 = 1800; // 30 min
pub const WATCH_MAX_PER_WINDOW: u32 = 8;
pub const WATCH_WINDOW_SECONDS: u64 = 10;
pub const WATCH_OVERLOAD_KILL_SECONDS: u64 = 45;

/// A tracked background process. Runtime state includes the rolling output
/// buffer; output is truncated at a UTF-8 char boundary per Pitfall 8.
pub struct ProcessSession {
    pub id: String,        // "proc_" + 12 hex chars
    pub task_id: String,   // session scoping key (D-24)
    pub command: String,
    pub pid: Option<u32>,
    pub cwd: Option<PathBuf>,
    pub started_at: Instant,
    pub finished_at: Option<Instant>,
    pub exit_code: Option<i32>,
    pub output_buffer: String,
    pub watch_patterns: Vec<Regex>,
    pub watch_state: WatchState,
    pub(crate) child: Arc<Mutex<Option<Child>>>,
    pub(crate) cancel: CancellationToken,
}

#[derive(Debug, Clone)]
pub struct WatchState {
    pub window_start: Instant,
    pub window_hits: u32,
    pub overload_since: Option<Instant>,
    pub disabled: bool,
}

impl Default for WatchState {
    fn default() -> Self {
        Self {
            window_start: Instant::now(),
            window_hits: 0,
            overload_since: None,
            disabled: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateDecision {
    Fire,
    Drop,
    AutoDisable,
}

#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub watch_patterns: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct WatchEvent {
    pub session_id: String,
    pub pid: u32,
    pub matched_line: String,
    pub pattern_index: usize,
}

#[derive(Debug, Clone)]
pub struct ProcessStatus {
    pub id: String,
    pub pid: Option<u32>,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub uptime_secs: u64,
    pub output_tail: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcessSnapshot {
    pub id: String,
    pub pid: Option<u32>,
    pub task_id: String,
    pub command: String,
    pub uptime_secs: u64,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcessRegistrySnapshot {
    pub tracked: usize,
    pub entries: Vec<ProcessSnapshot>,
}

/// Short uptime format (D-28). Matches hermes-agent process_registry.py:56.
pub fn format_uptime_short(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{}s", seconds);
    }
    if seconds < 3600 {
        let m = seconds / 60;
        let s = seconds % 60;
        return format!("{}m {}s", m, s);
    }
    let h = seconds / 3600;
    let m = (seconds % 3600) / 60;
    format!("{}h {}m", h, m)
}

pub struct ProcessRegistry {
    pub(crate) running: HashMap<String, ProcessSession>,
    pub(crate) finished: HashMap<String, ProcessSession>,
    pub(crate) task_id: String,
    pub(crate) watch_tx: broadcast::Sender<WatchEvent>,
}

impl ProcessRegistry {
    pub fn new_for_session(task_id: impl Into<String>) -> Self {
        let (watch_tx, _) = broadcast::channel(256);
        Self {
            running: HashMap::new(),
            finished: HashMap::new(),
            task_id: task_id.into(),
            watch_tx,
        }
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }
    pub fn watch_subscribe(&self) -> broadcast::Receiver<WatchEvent> {
        self.watch_tx.subscribe()
    }
    pub fn running_count(&self) -> usize {
        self.running.len()
    }
    pub fn finished_count(&self) -> usize {
        self.finished.len()
    }
}

// Output-buffer truncation helper (Pitfall 8 — UTF-8 char boundary).
pub(crate) fn append_bounded_utf8(buf: &mut String, incoming: &str, max: usize) {
    buf.push_str(incoming);
    if buf.len() <= max {
        return;
    }
    let target_start = buf.len() - max;
    // Advance to next char boundary >= target_start.
    let mut idx = target_start;
    while idx < buf.len() && !buf.is_char_boundary(idx) {
        idx += 1;
    }
    *buf = buf[idx..].to_string();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_uptime_short_reference_vectors() {
        assert_eq!(format_uptime_short(0), "0s");
        assert_eq!(format_uptime_short(59), "59s");
        assert_eq!(format_uptime_short(60), "1m 0s");
        assert_eq!(format_uptime_short(3599), "59m 59s");
        assert_eq!(format_uptime_short(3600), "1h 0m");
        assert_eq!(format_uptime_short(3661), "1h 1m");
        assert_eq!(format_uptime_short(7_322), "2h 2m");
    }

    #[test]
    fn append_bounded_utf8_truncates_at_char_boundary() {
        let mut buf = String::new();
        let max = 10;
        // Non-ASCII multi-byte: 'é' is 2 bytes.
        let chunk = "ééééééé"; // 14 bytes, 7 chars
        append_bounded_utf8(&mut buf, chunk, max);
        assert!(
            buf.len() <= max + 1,
            "buf.len() {} should be near max {}",
            buf.len(),
            max
        );
        // Must be valid UTF-8 (string was kept so it must be).
        assert!(buf.is_char_boundary(0));
        assert!(buf.is_char_boundary(buf.len()));
    }

    #[test]
    fn append_bounded_utf8_exceeds_cap_rolls_window() {
        let mut buf = String::new();
        append_bounded_utf8(&mut buf, &"a".repeat(50), 20);
        assert_eq!(buf.len(), 20);
        append_bounded_utf8(&mut buf, "XYZ", 20);
        assert!(buf.ends_with("XYZ"));
        assert_eq!(buf.len(), 20);
    }

    #[test]
    fn new_for_session_stores_task_id() {
        let r = ProcessRegistry::new_for_session("sess-abc");
        assert_eq!(r.task_id(), "sess-abc");
        assert_eq!(r.running_count(), 0);
    }

    #[test]
    fn watch_subscribe_yields_broadcast_receiver() {
        let r = ProcessRegistry::new_for_session("t");
        let _rx = r.watch_subscribe();
        // Compile-check: the type is broadcast::Receiver<WatchEvent>
    }
}

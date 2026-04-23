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

impl ProcessRegistry {
    /// Spawn a background process tracked by this registry.
    ///
    /// Returns the registry-assigned ID (`proc_<12hex>`). The caller (Plan 06)
    /// is expected to own the stdout/stderr drain tasks that feed
    /// `append_bounded_utf8` into the session's `output_buffer` and route
    /// matching lines through `rate_limit_check` before broadcasting
    /// `WatchEvent`. This method only establishes the tracked session.
    pub async fn spawn(&mut self, spec: SpawnSpec) -> anyhow::Result<String> {
        // LRU prune before accept (D-25).
        self.prune_lru_if_needed().await?;
        self.prune_finished_ttl();

        // Validate watch patterns up-front (fail fast per Pitfall 7).
        let mut compiled = Vec::with_capacity(spec.watch_patterns.len());
        for p in &spec.watch_patterns {
            compiled.push(
                Regex::new(p)
                    .map_err(|e| anyhow::anyhow!("invalid watch pattern {:?}: {}", p, e))?,
            );
        }

        // Build the command via tokio::process::Command.
        let parts = shell_words::split(&spec.command)
            .map_err(|e| anyhow::anyhow!("command parse: {}", e))?;
        let (prog, args) = parts
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("empty command"))?;
        let mut cmd = tokio::process::Command::new(prog);
        cmd.args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(false); // explicit cancel only (Pitfall 6)
        if let Some(ref cwd) = spec.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn()?;
        let pid = child.id();
        let cancel = CancellationToken::new();
        let id = format!("proc_{}", random_hex_12());

        // Plan 06 wiring: drain stdout/stderr off the Child here; for this plan
        // we take them so they're dropped (else the pipe buffers could fill).
        // The stdout-drain task lives in Plan 06 where the tool caller has the
        // join_handle for `append_bounded_utf8` + `rate_limit_check` routing.
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let _ = (stdout, stderr);

        let child_arc = Arc::new(Mutex::new(Some(child)));
        let session = ProcessSession {
            id: id.clone(),
            task_id: self.task_id.clone(),
            command: spec.command.clone(),
            pid,
            cwd: spec.cwd.clone(),
            started_at: Instant::now(),
            finished_at: None,
            exit_code: None,
            output_buffer: String::new(),
            watch_patterns: compiled.clone(),
            watch_state: WatchState::default(),
            child: child_arc.clone(),
            cancel: cancel.clone(),
        };
        self.running.insert(id.clone(), session);
        Ok(id)
    }

    /// Poll current status (D-25). Returns `None` if `id` is unknown.
    pub async fn poll(&self, id: &str) -> Option<ProcessStatus> {
        let s = self.running.get(id).or_else(|| self.finished.get(id))?;
        let uptime = s.started_at.elapsed().as_secs();
        let output_tail = tail_by_chars(&s.output_buffer, 2_000);
        Some(ProcessStatus {
            id: s.id.clone(),
            pid: s.pid,
            running: self.running.contains_key(id),
            exit_code: s.exit_code,
            uptime_secs: uptime,
            output_tail,
        })
    }

    /// Wait up to `timeout` for the process to exit. On exit, moves the
    /// session to `finished`. On timeout, leaves it running.
    pub async fn wait(
        &mut self,
        id: &str,
        timeout: Duration,
    ) -> anyhow::Result<ProcessStatus> {
        let child_arc = {
            let s = self
                .running
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("no such running process: {}", id))?;
            s.child.clone()
        };
        let exit_status = match tokio::time::timeout(timeout, async {
            let mut guard = child_arc.lock().await;
            if let Some(child) = guard.as_mut() {
                child.wait().await
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "already reaped",
                ))
            }
        })
        .await
        {
            Ok(Ok(status)) => Some(status),
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => None, // timeout
        };
        if let Some(status) = exit_status {
            let code = status.code();
            self.move_to_finished(id, code);
        }
        self.poll(id)
            .await
            .ok_or_else(|| anyhow::anyhow!("process vanished mid-wait"))
    }

    /// Kill a tracked process. Idempotent: unknown / already-finished IDs
    /// return `Ok(())`. Uses `Child::kill()` (SIGKILL on unix) — the registry
    /// is the final-resort reaper, matching hermes-agent semantics. A graceful
    /// SIGTERM→SIGKILL escalation with `nix::kill` is deferred to the caller
    /// (Plan 06) if/when graceful shutdown semantics are wired.
    pub async fn kill(&mut self, id: &str) -> anyhow::Result<()> {
        let (child_arc, cancel) = match self.running.get(id) {
            Some(s) => (s.child.clone(), s.cancel.clone()),
            None => return Ok(()), // idempotent — already gone
        };
        cancel.cancel();
        let mut guard = child_arc.lock().await;
        if let Some(child) = guard.as_mut() {
            // SIGKILL + reap. On unix this triggers waitpid internally via
            // tokio's child reaper — no zombie is left.
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        drop(guard);
        // Exit code unknown on forced kill; stored as None.
        self.move_to_finished(id, None);
        Ok(())
    }

    /// Last `tail_bytes` bytes of the buffer, rounded to the next UTF-8 char
    /// boundary. Returns `None` for unknown IDs.
    pub fn logs(&self, id: &str, tail_bytes: usize) -> Option<String> {
        let s = self.running.get(id).or_else(|| self.finished.get(id))?;
        if s.output_buffer.len() <= tail_bytes {
            return Some(s.output_buffer.clone());
        }
        let start = s.output_buffer.len() - tail_bytes;
        let mut idx = start;
        while idx < s.output_buffer.len() && !s.output_buffer.is_char_boundary(idx) {
            idx += 1;
        }
        Some(s.output_buffer[idx..].to_string())
    }

    /// D-24: kill every running child; await reaping; retain finished entries
    /// for TTL.
    pub async fn drain_and_kill(&mut self) -> anyhow::Result<()> {
        let ids: Vec<String> = self.running.keys().cloned().collect();
        for id in ids {
            let _ = self.kill(&id).await; // best-effort per-entry
        }
        Ok(())
    }

    /// D-24 session-scoped variant (called from `on_session_end`).
    pub async fn drain_and_kill_session(&mut self, task_id: &str) -> anyhow::Result<()> {
        if self.task_id != task_id {
            return Ok(());
        }
        self.drain_and_kill().await
    }

    /// D-28: snapshot for `hermes status`. Sorted by uptime descending
    /// (longest-running first). Finished entries include their exit code.
    pub fn snapshot(&self) -> ProcessRegistrySnapshot {
        let mut entries: Vec<_> = self
            .running
            .values()
            .chain(self.finished.values())
            .map(|s| ProcessSnapshot {
                id: s.id.clone(),
                pid: s.pid,
                task_id: s.task_id.clone(),
                command: s.command.clone(),
                uptime_secs: s.started_at.elapsed().as_secs(),
                exit_code: s.exit_code,
            })
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.uptime_secs));
        ProcessRegistrySnapshot {
            tracked: entries.len(),
            entries,
        }
    }

    /// Broadcast `event` to any `watch_subscribe` receivers. Called from
    /// Plan 06 after `rate_limit_check` returns `Fire`.
    pub fn broadcast_watch_event(&self, event: WatchEvent) {
        // `send` errors only when there are zero active receivers; safe to ignore.
        let _ = self.watch_tx.send(event);
    }

    // ---- internal helpers ----

    fn move_to_finished(&mut self, id: &str, exit_code: Option<i32>) {
        if let Some(mut s) = self.running.remove(id) {
            s.finished_at = Some(Instant::now());
            s.exit_code = exit_code;
            self.finished.insert(s.id.clone(), s);
        }
    }

    async fn prune_lru_if_needed(&mut self) -> anyhow::Result<()> {
        if self.running.len() < MAX_PROCESSES {
            return Ok(());
        }
        // Find oldest-started entry.
        let oldest_id = self
            .running
            .values()
            .min_by_key(|s| s.started_at)
            .map(|s| s.id.clone());
        if let Some(id) = oldest_id {
            let _ = self.kill(&id).await;
        }
        Ok(())
    }

    /// Drop finished entries whose `finished_at` is older than
    /// `FINISHED_TTL_SECONDS`. Called automatically on `spawn`; also
    /// exposed for targeted tests and for the snapshot call-path.
    pub fn prune_finished_ttl(&mut self) {
        let cutoff = Duration::from_secs(FINISHED_TTL_SECONDS);
        self.finished.retain(|_, s| {
            s.finished_at.map(|t| t.elapsed() < cutoff).unwrap_or(true)
        });
    }

    /// Test-only: insert a pre-built finished session without spawning a real
    /// child. Integration tests use this to pre-populate the TTL bucket.
    #[doc(hidden)]
    pub fn insert_fake_finished(&mut self, s: ProcessSession) {
        self.finished.insert(s.id.clone(), s);
    }

    /// Test-only: insert a pre-built running session without spawning a real
    /// child. Integration tests use this for rate-limiter / LRU harnesses.
    #[doc(hidden)]
    pub fn insert_fake_running(&mut self, s: ProcessSession) {
        self.running.insert(s.id.clone(), s);
    }
}

/// Test-only builder for `ProcessSession`. Not part of the public runtime
/// surface — exists so integration tests can construct sessions without a
/// real `tokio::process::Child`.
#[doc(hidden)]
pub fn fake_process_session(
    id: impl Into<String>,
    task_id: impl Into<String>,
    command: impl Into<String>,
    pid: Option<u32>,
    started_at: Instant,
    finished_at: Option<Instant>,
    exit_code: Option<i32>,
) -> ProcessSession {
    ProcessSession {
        id: id.into(),
        task_id: task_id.into(),
        command: command.into(),
        pid,
        cwd: None,
        started_at,
        finished_at,
        exit_code,
        output_buffer: String::new(),
        watch_patterns: vec![],
        watch_state: WatchState::default(),
        child: Arc::new(Mutex::new(None)),
        cancel: CancellationToken::new(),
    }
}

fn tail_by_chars(s: &str, n: usize) -> String {
    if s.is_empty() || n == 0 {
        return String::new();
    }
    // Find a char boundary such that the resulting suffix is <= n bytes.
    let bytes = s.len();
    if bytes <= n {
        return s.to_string();
    }
    let mut idx = bytes - n;
    while idx < bytes && !s.is_char_boundary(idx) {
        idx += 1;
    }
    s[idx..].to_string()
}

fn random_hex_12() -> String {
    use std::time::SystemTime;
    let ns = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    // Fast non-crypto hash of timestamp + pid for within-session uniqueness.
    let pid = std::process::id() as u64;
    let mix = ns
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(pid)
        .wrapping_add(ns.wrapping_shl(13));
    format!("{:012x}", mix & 0x0000_FFFF_FFFF_FFFF)
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

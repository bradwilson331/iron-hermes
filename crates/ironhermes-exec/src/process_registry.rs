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
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdout};
use tokio::sync::{broadcast, Mutex, RwLock};
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
    /// Plan 21.7-06: stdout/stderr pipes kept on the session until a caller
    /// invokes `start_output_drain`; the drain tasks take these out and feed
    /// `ingest_output` as bytes arrive. `None` indicates the pipes were
    /// already handed to a drain task (or the child was spawned without them).
    pub(crate) stdout_pipe: Arc<Mutex<Option<ChildStdout>>>,
    pub(crate) stderr_pipe: Arc<Mutex<Option<ChildStderr>>>,
}

/// Watch-pattern rate-limiter state. Timing uses `tokio::time::Instant`
/// (not `std::time::Instant`) so tests can drive the window via
/// `tokio::time::pause()` + `advance()`.
#[derive(Debug, Clone)]
pub struct WatchState {
    pub window_start: tokio::time::Instant,
    pub window_hits: u32,
    pub overload_since: Option<tokio::time::Instant>,
    pub disabled: bool,
}

impl Default for WatchState {
    fn default() -> Self {
        Self {
            window_start: tokio::time::Instant::now(),
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

        // Plan 21.7-06: KEEP stdout/stderr on the session so a subsequent
        // `start_output_drain(reg, &id)` call can take them and spawn the
        // drain tasks that feed `ingest_output`. Dropping them here would
        // fill the kernel pipe buffer and deadlock the child once it wrote
        // past ~64KB. If the caller never wires a drain, the pipes stay
        // readable on the session until kill/drain_and_kill reaps the child.
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

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
            stdout_pipe: Arc::new(Mutex::new(stdout)),
            stderr_pipe: Arc::new(Mutex::new(stderr)),
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

    /// Plan 21.7-06: Feed a chunk of child output (already decoded UTF-8-safe or
    /// replaced) into the process's rolling buffer, scan for watch-pattern
    /// matches, consult the rate limiter, and broadcast matched events on
    /// `RateDecision::Fire`. 9+ matches per 10s window become `Drop` (with
    /// tracing::warn) and 45s sustained cap latches `AutoDisable`.
    ///
    /// Called by the stdout/stderr drain tasks spawned in
    /// `start_output_drain`. No-op for unknown `id` (process already drained).
    pub fn ingest_output(&mut self, id: &str, chunk: &str) {
        // Clone watch patterns up-front so we can release the &mut borrow on
        // self.running before invoking rate_limit_check (which also needs
        // &mut self). Empty chunk is a no-op.
        if chunk.is_empty() {
            return;
        }
        let patterns_snapshot: Vec<Regex> = match self.running.get(id) {
            Some(s) => s.watch_patterns.clone(),
            None => return,
        };

        // Always append to the rolling buffer, whether patterns are configured
        // or not — logs() consumers need the raw stream.
        if let Some(s) = self.running.get_mut(id) {
            append_bounded_utf8(&mut s.output_buffer, chunk, MAX_OUTPUT_CHARS);
        }

        if patterns_snapshot.is_empty() {
            return;
        }

        // Scan each line for the first matching pattern; forward through the
        // rate limiter and broadcast on Fire. One match per line is enough
        // (hermes-agent parity: we don't fire multiple events for one line).
        for line in chunk.lines() {
            for (idx, pat) in patterns_snapshot.iter().enumerate() {
                if pat.is_match(line) {
                    let decision = self.rate_limit_check(id);
                    if matches!(decision, RateDecision::Fire) {
                        let pid_opt = self.running.get(id).and_then(|s| s.pid);
                        if let Some(pid) = pid_opt {
                            let event = WatchEvent {
                                session_id: id.to_string(),
                                pid,
                                matched_line: line.to_string(),
                                pattern_index: idx,
                            };
                            self.broadcast_watch_event(event);
                        }
                    }
                    break;
                }
            }
        }
    }

    /// Plan 21.7-06: Take the stdout/stderr pipes from a tracked session and
    /// spawn two background drain tasks that read lines into
    /// `ingest_output` until EOF. The pipes are removed from the session on
    /// first call; subsequent calls are no-ops.
    ///
    /// Takes `Arc<RwLock<Self>>` so the drain tasks can `write()` a lock
    /// and call `ingest_output` for each line received. The caller (tool
    /// layer) passes `reg.clone()` after `spawn()` returns.
    ///
    /// The tasks exit cleanly on EOF (child stdout/stderr closes on exit).
    /// No `JoinHandle` is returned — the tasks are fire-and-forget; shutdown
    /// happens naturally via EOF when `drain_and_kill` kills the child.
    pub async fn start_output_drain(
        reg: Arc<RwLock<Self>>,
        id: &str,
    ) -> anyhow::Result<()> {
        let (stdout_pipe_arc, stderr_pipe_arc) = {
            let r = reg.read().await;
            let s = r
                .running
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("no such running process: {}", id))?;
            (s.stdout_pipe.clone(), s.stderr_pipe.clone())
        };

        // Take ownership of each pipe (if still present).
        let stdout_opt = {
            let mut g = stdout_pipe_arc.lock().await;
            g.take()
        };
        let stderr_opt = {
            let mut g = stderr_pipe_arc.lock().await;
            g.take()
        };

        if let Some(stdout) = stdout_opt {
            let reg_clone = reg.clone();
            let id_owned = id.to_string();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout).lines();
                loop {
                    match reader.next_line().await {
                        Ok(Some(line)) => {
                            // Append line + newline so the buffer preserves
                            // line boundaries for pattern scanning + logs tail.
                            let mut r = reg_clone.write().await;
                            let mut chunk = line;
                            chunk.push('\n');
                            r.ingest_output(&id_owned, &chunk);
                        }
                        Ok(None) => break, // EOF — child closed stdout
                        Err(e) => {
                            tracing::debug!(
                                target: "ironhermes_exec::process_registry",
                                id = %id_owned,
                                error = %e,
                                "stdout drain read error; ending task"
                            );
                            break;
                        }
                    }
                }
            });
        }

        if let Some(stderr) = stderr_opt {
            let reg_clone = reg.clone();
            let id_owned = id.to_string();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                loop {
                    match reader.next_line().await {
                        Ok(Some(line)) => {
                            let mut r = reg_clone.write().await;
                            let mut chunk = line;
                            chunk.push('\n');
                            r.ingest_output(&id_owned, &chunk);
                        }
                        Ok(None) => break,
                        Err(e) => {
                            tracing::debug!(
                                target: "ironhermes_exec::process_registry",
                                id = %id_owned,
                                error = %e,
                                "stderr drain read error; ending task"
                            );
                            break;
                        }
                    }
                }
            });
        }

        Ok(())
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

    /// Test-only: insert a bare fake running session identified by `id`
    /// with the given `pid`. Shorthand for the rate-limit integration tests.
    #[doc(hidden)]
    pub fn insert_fake_running_minimal(&mut self, id: &str, pid: u32) {
        let s = fake_process_session(
            id,
            self.task_id.clone(),
            "test",
            Some(pid),
            Instant::now(),
            None,
            None,
        );
        self.running.insert(s.id.clone(), s);
    }

    /// Test-only accessor for watch-state introspection.
    #[doc(hidden)]
    pub fn watch_state_for(&self, id: &str) -> Option<&WatchState> {
        self.running.get(id).map(|s| &s.watch_state)
    }

    /// D-27 / Pitfall 7: Rate-limit decision for a single watch-pattern match
    /// on `id`. Mutates the entry's `watch_state` in place.
    ///
    /// Semantics (hermes-agent process_registry.py:162-250):
    ///  - Maintain a rolling `WATCH_WINDOW_SECONDS` (10s) window per process.
    ///  - First `WATCH_MAX_PER_WINDOW` (8) calls in the window return `Fire`.
    ///  - 9th+ calls in the same window return `Drop` and emit `tracing::warn`.
    ///  - If the window stays saturated for `WATCH_OVERLOAD_KILL_SECONDS` (45s)
    ///    consecutively, latch `watch_state.disabled = true`, emit a
    ///    `tracing::warn` with `"watch overload disabled"`, and return
    ///    `AutoDisable` exactly once.
    ///  - Once disabled, every subsequent call returns `Drop` silently.
    ///  - Scope is per-process — disabling one entry does not affect peers.
    pub fn rate_limit_check(&mut self, id: &str) -> RateDecision {
        // tokio::time::Instant respects tokio::time::pause()/advance() so the
        // rate-limit tests can drive rolling-window transitions deterministically.
        let now = tokio::time::Instant::now();
        let s = match self.running.get_mut(id) {
            Some(s) => s,
            None => return RateDecision::Drop, // process gone — drop silently
        };
        let pid = s.pid;

        // Already disabled — every call is Drop (AutoDisable was already emitted).
        if s.watch_state.disabled {
            return RateDecision::Drop;
        }

        // Roll the window when WATCH_WINDOW_SECONDS elapsed since window_start.
        // IMPORTANT: `overload_since` must persist across window rolls so that
        // sustained-cap (45s) detection accumulates. It is cleared only when
        // a full window passes WITHOUT exceeding the cap (see the end branch).
        if now.duration_since(s.watch_state.window_start)
            >= Duration::from_secs(WATCH_WINDOW_SECONDS)
        {
            // If the previous window stayed under cap, the overload has cooled;
            // reset the sustained-cap anchor.
            if s.watch_state.window_hits <= WATCH_MAX_PER_WINDOW {
                s.watch_state.overload_since = None;
            }
            s.watch_state.window_start = now;
            s.watch_state.window_hits = 0;
        }

        s.watch_state.window_hits += 1;

        if s.watch_state.window_hits <= WATCH_MAX_PER_WINDOW {
            return RateDecision::Fire;
        }

        // Over cap — record the start of the sustained-cap condition.
        if s.watch_state.overload_since.is_none() {
            s.watch_state.overload_since = Some(now);
        }
        let dropped_count = s.watch_state.window_hits - WATCH_MAX_PER_WINDOW;
        tracing::warn!(
            target: "ironhermes_exec::process_registry",
            pid = ?pid,
            dropped_count,
            "watch pattern backpressure: dropped {} match(es) in last {}s",
            dropped_count,
            WATCH_WINDOW_SECONDS
        );

        // Check sustained-overload auto-disable threshold.
        if let Some(overload_since) = s.watch_state.overload_since {
            if now.duration_since(overload_since)
                >= Duration::from_secs(WATCH_OVERLOAD_KILL_SECONDS)
            {
                s.watch_state.disabled = true;
                tracing::warn!(
                    target: "ironhermes_exec::process_registry",
                    pid = ?pid,
                    "watch overload disabled after {}s sustained at cap",
                    WATCH_OVERLOAD_KILL_SECONDS
                );
                return RateDecision::AutoDisable;
            }
        }
        RateDecision::Drop
    }
}

/// Plan 21.7-07 (D-26): newtype wrapper around `Arc<RwLock<ProcessRegistry>>`
/// that implements `ProcessRegistrySnapshotHandle`. Newtype required by Rust's
/// orphan rule (can't impl foreign trait on foreign type `Arc<RwLock<_>>`).
///
/// Sync trait methods bridge to the async `tokio::sync::RwLock` via
/// `block_in_place` + `Handle::current().block_on` — same pattern used in
/// `ironhermes-core/src/commands/handlers.rs` for `/models refresh`.
#[derive(Clone)]
pub struct ProcessRegistryHandle(pub Arc<tokio::sync::RwLock<ProcessRegistry>>);

impl ProcessRegistryHandle {
    pub fn new(reg: Arc<tokio::sync::RwLock<ProcessRegistry>>) -> Self {
        Self(reg)
    }
}

impl ironhermes_core::commands::context::ProcessRegistrySnapshotHandle for ProcessRegistryHandle {
    fn tracked(&self) -> usize {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { self.0.read().await.snapshot().tracked })
        })
    }

    fn snapshot_json(&self) -> serde_json::Value {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let snap = self.0.read().await.snapshot();
                serde_json::to_value(&snap).unwrap_or_else(|_| serde_json::json!({}))
            })
        })
    }

    fn drain_and_kill<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let _ = self.0.write().await.drain_and_kill().await;
        })
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
        stdout_pipe: Arc::new(Mutex::new(None)),
        stderr_pipe: Arc::new(Mutex::new(None)),
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
//
// Plan 21.7-06: promoted from `pub(crate)` → `pub` so stdout/stderr drain
// tasks spawned outside this module (e.g., Plan 06 tool layer) can reuse
// the canonical char-boundary-safe append without re-implementing it.
pub fn append_bounded_utf8(buf: &mut String, incoming: &str, max: usize) {
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

    /// Plan 21.7-06 Task 6-01 — E-04: ingest_output broadcasts the first 8
    /// matching lines in a 10s window and drops the rest (9+). The rolling
    /// buffer stays bounded. Unknown process IDs are a no-op (no panic).
    #[tokio::test(start_paused = true)]
    async fn ingest_output_broadcasts_first_eight_matches_then_drops() {
        let mut reg = ProcessRegistry::new_for_session("t-ingest");
        reg.insert_fake_running_minimal("proc_a", 42);
        if let Some(s) = reg.running.get_mut("proc_a") {
            s.watch_patterns.push(Regex::new(r"MATCH").unwrap());
        }

        let mut rx = reg.watch_subscribe();

        let chunk = "MATCH 1\nMATCH 2\nMATCH 3\nMATCH 4\nMATCH 5\nMATCH 6\nMATCH 7\nMATCH 8\nMATCH 9\nMATCH 10\nMATCH 11\nMATCH 12\n";
        reg.ingest_output("proc_a", chunk);

        let mut fire_count = 0;
        while rx.try_recv().is_ok() {
            fire_count += 1;
        }
        assert_eq!(fire_count, 8, "E-04: first 8 fire, rest drop");

        // Rolling buffer must contain the full ingested chunk (well below MAX_OUTPUT_CHARS).
        let s = reg.running.get("proc_a").unwrap();
        assert!(
            s.output_buffer.contains("MATCH 12"),
            "output_buffer must retain full ingested chunk: len={}",
            s.output_buffer.len()
        );
        assert!(s.output_buffer.len() <= MAX_OUTPUT_CHARS);
    }

    /// Unknown id is a silent no-op (drain tasks may race drain_and_kill).
    #[tokio::test(start_paused = true)]
    async fn ingest_output_unknown_id_is_noop() {
        let mut reg = ProcessRegistry::new_for_session("t-miss");
        reg.ingest_output("proc_ghost", "hello\n");
        // No panic, no broadcast channel reference needed.
    }

    /// Empty chunk is a no-op (drain task may send 0-byte reads).
    #[tokio::test(start_paused = true)]
    async fn ingest_output_empty_chunk_is_noop() {
        let mut reg = ProcessRegistry::new_for_session("t-empty");
        reg.insert_fake_running_minimal("proc_a", 42);
        reg.ingest_output("proc_a", "");
        let s = reg.running.get("proc_a").unwrap();
        assert!(s.output_buffer.is_empty());
    }

    /// No watch pattern configured → appends to buffer but no broadcast.
    #[tokio::test(start_paused = true)]
    async fn ingest_output_no_patterns_appends_only() {
        let mut reg = ProcessRegistry::new_for_session("t-quiet");
        reg.insert_fake_running_minimal("proc_a", 42);
        let mut rx = reg.watch_subscribe();
        reg.ingest_output("proc_a", "some data\nmore data\n");
        assert!(rx.try_recv().is_err(), "no broadcast without patterns");
        let s = reg.running.get("proc_a").unwrap();
        assert!(s.output_buffer.contains("some data"));
        assert!(s.output_buffer.contains("more data"));
    }
}

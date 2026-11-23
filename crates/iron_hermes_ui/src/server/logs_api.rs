//! Phase 54.4 Plan 04 (D-08 / D-24): the server read path for the Logs screen.
//!
//! One `#[server]` fn, [`read_log_tail`], returns a bounded, incremental tail
//! of either the server log (`$IRONHERMES_HOME/logs/web.log.YYYY-MM-DD`) or the
//! audit log (`$IRONHERMES_HOME/logs/audit.jsonl`). `audit.jsonl` is
//! append-only and never rotated, so every limit is enforced here on the
//! server, never trusted from the client.
//!
//! # Security design (T-54.4-11 / T-54.4-12 / T-54.4-13)
//!
//! - **No client-supplied path.** [`LogSource`] is a two-variant enum; the
//!   directory is the server-resolved `get_hermes_home().join("logs")`; the
//!   server log file name is picked by the server (newest `web.log.*`).
//!   [`LogCursor::file_key`] is only ever *compared* against the
//!   server-resolved name, never joined onto a path.
//! - **Exact prefix.** Only names starting with `web.log.` are candidates, so
//!   the sibling `web-access.log.*` files and the `kanban/` and `bot/`
//!   subtrees are never read (D-08).
//! - **Symlinks refused.** A candidate must satisfy
//!   `symlink_metadata().file_type().is_file()`, checked again at read time
//!   (narrowing the TOCTOU window). On unix the file is then opened with
//!   `O_NOFOLLOW | O_NONBLOCK` ([`open_regular_no_follow`]): the kernel itself
//!   refuses a symlink swapped in after the lstat, and a swapped-in FIFO cannot
//!   block the thread. The `fstat` on the opened handle must still report a
//!   regular file. (Without `O_NOFOLLOW`, `File::open` follows the link and
//!   the `fstat` would describe the TARGET, which proves nothing.)
//! - **Bounded reads.** Seek-from-end 256 KiB window ([`MAX_TAIL_BYTES`]), at
//!   most [`MAX_LINES`] lines, at most [`MAX_LINE_BYTES`] per line (cut on a
//!   UTF-8 boundary); `max_lines` is clamped to `1..=MAX_LINES`. The file is
//!   never read whole.
//! - **No path in errors.** Error strings are fixed; no absolute path is ever
//!   interpolated.
//! - **No agent attribution (D-24).** Neither source carries an agent: the
//!   DTOs have no agent field and the reader applies no text-match filter.
//!
//! Unconditionally declared like `profile_activation_api`: the `#[server]`
//! macro handles the client/server split, and native-only internals are gated
//! `#[cfg(feature = "server")]` (the wasm client never compiles them). It is
//! wrapped by `require_auth` like every registered server fn. All file I/O
//! runs in `tokio::task::spawn_blocking` (`block_in_place` panics in the
//! per-connection `LocalSet`).
//!
//! # Polling cursor
//!
//! A cursor `(file_key, offset)` addresses the byte just past the last byte
//! returned. A poll with a cursor for the same file returns only bytes
//! appended after `offset`; a shrunk file or a new daily file yields
//! `reset = true` with a fresh tail. A trailing unterminated segment is
//! returned as a line (tracing writes whole lines per write, so a split line
//! across two polls is not a practical concern).

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Which log file the client wants. A closed set: the client can never name a
/// file or path.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogSource {
    /// The newest `web.log.YYYY-MM-DD` daily server log.
    Server,
    /// `audit.jsonl`, parsed into [`AuditRowWire`] rows.
    Audit,
}

/// Polling cursor: the server-resolved file name plus the byte offset just
/// past the last byte previously returned.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LogCursor {
    /// Compared against the server-resolved file name; never joined onto a path.
    pub file_key: String,
    pub offset: u64,
}

/// One `audit.jsonl` row on the wire. `args` is already redacted and
/// truncated at write time (`ironhermes-core::audit`); it is not re-redacted
/// here.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AuditRowWire {
    pub ts: String,
    pub kind: String,
    pub tool: String,
    pub server: Option<String>,
    pub session: String,
    pub surface: String,
    pub decision: String,
    pub resolution: String,
    pub reason: String,
    pub args: String,
}

/// One displayed log line.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum LogLine {
    /// A server log line plus its detected tracing level, if any.
    Server { text: String, level: Option<String> },
    /// A parsed audit row.
    Audit(AuditRowWire),
    /// A line that could not be parsed (never dropped, never an error).
    Raw { text: String },
}

/// One page of a tail read.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LogTailPage {
    pub lines: Vec<LogLine>,
    /// Cursor to pass on the next poll; `None` when the file is absent.
    pub cursor: Option<LogCursor>,
    /// The client should discard what it holds: the file shrank or rotated.
    pub reset: bool,
    /// Older content was omitted (window, line-count, or partial-line drop).
    pub truncated: bool,
}

/// Maximum bytes read per request, seeked from the end of the file.
pub const MAX_TAIL_BYTES: u64 = 256 * 1024;
/// Maximum bytes of one returned line (cut on a UTF-8 boundary).
pub const MAX_LINE_BYTES: usize = 4096;
/// Maximum lines returned per request.
pub const MAX_LINES: usize = 500;

/// Result of a raw tail read.
#[cfg(feature = "server")]
struct TailRead {
    lines: Vec<String>,
    end: u64,
    reset: bool,
    truncated: bool,
}

/// Cut `s` to at most `max` bytes at the greatest char boundary `<= max`.
#[cfg(feature = "server")]
pub(crate) fn truncate_line(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s[..cut].to_string()
}

/// The newest regular `web.log.*` file in `dir`, as `(path, file_name)`.
///
/// The exact `web.log.` prefix excludes `web-access.log.*`; the
/// `symlink_metadata` regular-file requirement excludes symlinks and
/// directories; dated names sort lexicographically.
#[cfg(feature = "server")]
fn newest_server_log(dir: &std::path::Path) -> Option<(std::path::PathBuf, String)> {
    let mut best: Option<(std::path::PathBuf, String)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !name.starts_with("web.log.") {
            continue;
        }
        let path = entry.path();
        let is_regular = std::fs::symlink_metadata(&path)
            .map(|m| m.file_type().is_file())
            .unwrap_or(false);
        if !is_regular {
            continue;
        }
        if best.as_ref().is_none_or(|(_, b)| name > *b) {
            best = Some((path, name));
        }
    }
    best
}

/// Open `path` for reading without following a final symlink, and confirm the
/// opened handle is a regular file. Returns the handle and its length.
///
/// On unix the open uses `O_NOFOLLOW` (a symlink at `path` fails with `ELOOP`)
/// and `O_NONBLOCK` (opening a swapped-in FIFO cannot block the blocking
/// thread; reads of a regular file are unaffected). Elsewhere it falls back to
/// a plain open plus the `fstat` check. The error carries no path.
#[cfg(feature = "server")]
fn open_regular_no_follow(path: &std::path::Path) -> std::io::Result<(std::fs::File, u64)> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "not a regular file",
        ));
    }
    Ok((file, meta.len()))
}

/// Read a bounded tail of `path`.
///
/// Refuses a non-regular file (symlink, directory). The last
/// [`MAX_TAIL_BYTES`] are read and, when that window starts mid-file, the
/// first (partial) line is dropped. Never reads the file whole.
#[cfg(feature = "server")]
fn tail_lines(
    path: &std::path::Path,
    file_key: &str,
    cursor: Option<&LogCursor>,
    max_lines: usize,
) -> std::io::Result<TailRead> {
    use std::io::{Read, Seek, SeekFrom};

    let refuse = || std::io::Error::new(std::io::ErrorKind::PermissionDenied, "not a regular file");
    // Re-check at read time (TOCTOU-narrowing); the open itself refuses a
    // symlink and the handle is fstat-checked.
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(refuse());
    }
    let (mut file, len) = open_regular_no_follow(path)?;

    // A cursor is usable only for the same server-resolved file and only if
    // the file has not shrunk below it; otherwise fall back to a fresh tail
    // and tell the client to discard what it holds.
    let (resume_at, reset) = match cursor {
        Some(c) if c.file_key == file_key && c.offset <= len => (Some(c.offset), false),
        Some(_) => (None, true),
        None => (None, false),
    };
    let window_start = len.saturating_sub(MAX_TAIL_BYTES);
    let (start, drop_first) = match resume_at {
        // The cursor sits at the end of the previous read, i.e. on a line
        // start, so the first line is complete.
        Some(off) if off >= window_start => (off, false),
        // The gap exceeded the window: read only the window and drop the
        // partial first line.
        Some(_) => (window_start, true),
        None => (window_start, window_start > 0),
    };
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    // `take` bounds the read even if the file grew after `metadata()`.
    file.take(len - start).read_to_end(&mut buf)?;
    let end = start + buf.len() as u64;

    let mut segments: Vec<&[u8]> = buf.split(|b| *b == b'\n').collect();
    // A trailing newline leaves an empty last segment.
    if segments.last().is_some_and(|s| s.is_empty()) {
        segments.pop();
    }
    if drop_first && !segments.is_empty() {
        segments.remove(0);
    }
    let mut truncated = drop_first;
    if segments.len() > max_lines {
        segments.drain(..segments.len() - max_lines);
        truncated = true;
    }
    let lines = segments
        .into_iter()
        .map(|seg| {
            let seg = seg.strip_suffix(b"\r").unwrap_or(seg);
            truncate_line(&String::from_utf8_lossy(seg), MAX_LINE_BYTES)
        })
        .collect();
    Ok(TailRead {
        lines,
        end,
        reset,
        truncated,
    })
}

/// Parse one `audit.jsonl` line. A JSON object becomes a typed row (missing
/// string fields default to `""`, `server` to `None`); anything else is kept
/// as a raw line, never dropped and never an error. `args` is not re-redacted:
/// redaction happens at write time.
#[cfg(feature = "server")]
fn parse_audit_line(text: String) -> LogLine {
    let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(&text)
    else {
        return LogLine::Raw { text };
    };
    let field = |key: &str| -> String {
        obj.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    LogLine::Audit(AuditRowWire {
        ts: field("ts"),
        kind: field("kind"),
        tool: field("tool"),
        server: obj
            .get("server")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        session: field("session"),
        surface: field("surface"),
        decision: field("decision"),
        resolution: field("resolution"),
        reason: field("reason"),
        args: field("args"),
    })
}

/// Wrap a server log line with its tracing level, if one appears as a
/// whitespace-delimited token.
#[cfg(feature = "server")]
fn classify_server_line(text: String) -> LogLine {
    const LEVELS: [&str; 5] = ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"];
    let level = text
        .split_whitespace()
        .find(|tok| LEVELS.contains(tok))
        .map(str::to_string);
    LogLine::Server { text, level }
}

/// Synchronous body of [`read_log_tail`]; runs inside `spawn_blocking`.
#[cfg(feature = "server")]
fn read_log_tail_blocking(
    logs_dir: &std::path::Path,
    source: LogSource,
    max_lines: usize,
    cursor: Option<&LogCursor>,
) -> Result<LogTailPage, &'static str> {
    let empty = || LogTailPage {
        lines: Vec::new(),
        cursor: None,
        reset: false,
        truncated: false,
    };
    let (path, file_key, err, parse): (_, _, _, fn(String) -> LogLine) = match source {
        LogSource::Server => {
            let Some((path, file_key)) = newest_server_log(logs_dir) else {
                return Ok(empty());
            };
            (
                path,
                file_key,
                "couldn't read the server log",
                classify_server_line,
            )
        }
        LogSource::Audit => (
            logs_dir.join("audit.jsonl"),
            "audit.jsonl".to_string(),
            "couldn't read the audit log",
            parse_audit_line,
        ),
    };
    let read = match tail_lines(&path, &file_key, cursor, max_lines) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(empty()),
        Err(_) => return Err(err),
    };
    Ok(LogTailPage {
        lines: read.lines.into_iter().map(parse).collect(),
        cursor: Some(LogCursor {
            file_key,
            offset: read.end,
        }),
        reset: read.reset,
        truncated: read.truncated,
    })
}

/// Read a bounded tail of the server log or the audit log (D-08).
///
/// POST (`#[server]`, never `#[get]`: it takes arguments). `max_lines` is
/// clamped to `1..=MAX_LINES`. See the module docs for the full limit set.
#[server]
pub async fn read_log_tail(
    source: LogSource,
    max_lines: u32,
    cursor: Option<LogCursor>,
) -> Result<LogTailPage, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let max_lines = (max_lines as usize).clamp(1, MAX_LINES);
        let logs_dir = ironhermes_core::constants::get_hermes_home().join("logs");
        let result = tokio::task::spawn_blocking(move || {
            read_log_tail_blocking(&logs_dir, source, max_lines, cursor.as_ref())
        })
        .await
        .map_err(|_| ServerFnError::new("couldn't read the log"))?;
        result.map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (source, max_lines, cursor);
        Err(ServerFnError::new(
            "read_log_tail unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// RAII guard that sets `IRONHERMES_HOME` and restores the previous value
    /// on drop. Duplicated from `profile_api.rs`'s own `ScopedEnv` (each
    /// `#[cfg(test)]` module is its own namespace; the crate's sanctioned
    /// "duplicate the guard" precedent).
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: tests run with --test-threads=1; no concurrent env access.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: tests run with --test-threads=1; no concurrent env access.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    fn home(dir: &tempfile::TempDir) -> ScopedEnv {
        ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        )
    }

    fn logs_dir(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let p = dir.path().join("logs");
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
    }

    fn server_texts(page: &LogTailPage) -> Vec<String> {
        page.lines
            .iter()
            .map(|l| match l {
                LogLine::Server { text, .. } => text.clone(),
                other => panic!("expected a Server line, got {other:?}"),
            })
            .collect()
    }

    #[tokio::test]
    async fn log_tail_server_returns_newest_daily_file() {
        let dir = tempfile::tempdir().unwrap();
        let _g = home(&dir);
        let logs = logs_dir(&dir);
        write(&logs.join("web.log.2026-10-07"), "old1\nold2\n");
        write(&logs.join("web.log.2026-10-08"), "a\nb\nc\n");

        let page = read_log_tail(LogSource::Server, 100, None).await.unwrap();
        assert_eq!(server_texts(&page), vec!["a", "b", "c"]);
        let cursor = page.cursor.expect("cursor present");
        assert_eq!(cursor.file_key, "web.log.2026-10-08");
        assert_eq!(cursor.offset, 6);
        assert!(!page.reset);
        assert!(!page.truncated);
    }

    #[test]
    fn log_tail_newest_server_log_ignores_access_logs_and_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let logs = logs_dir(&dir);
        write(&logs.join("web.log.2026-10-07"), "old\n");
        write(&logs.join("web.log.2026-10-08"), "new\n");
        write(&logs.join("web-access.log.2026-10-09"), "GET /\n");
        fs::create_dir_all(logs.join("kanban")).unwrap();
        write(&logs.join("kanban").join("web.log.2026-10-12"), "worker\n");
        // A directory that happens to carry the prefix is not a log file.
        fs::create_dir_all(logs.join("web.log.2026-10-11")).unwrap();

        let (path, name) = newest_server_log(&logs).expect("a server log exists");
        assert_eq!(name, "web.log.2026-10-08");
        assert_eq!(path, logs.join("web.log.2026-10-08"));
    }

    #[test]
    fn log_tail_drops_partial_first_line_and_caps_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web.log.2026-10-08");
        // 600 KiB of 12-byte numbered lines.
        let total_lines = 600 * 1024 / 12;
        let mut body = String::new();
        for i in 0..total_lines {
            body.push_str(&format!("line-{i:06}\n"));
        }
        write(&path, &body);
        let len = body.len() as u64;
        let last = format!("line-{:06}", total_lines - 1);

        // Unbounded line count: only the 256 KiB window limits the read.
        let wide = tail_lines(&path, "web.log.2026-10-08", None, usize::MAX).unwrap();
        assert!(wide.truncated, "a mid-file window is a truncated read");
        assert_eq!(wide.end, len);
        assert_eq!(wide.lines.last().unwrap(), &last);
        let first = wide.lines.first().unwrap();
        assert_eq!(
            first.len(),
            11,
            "first line must be complete, got {first:?}"
        );
        assert!(first.starts_with("line-"));
        let consumed: usize = wide.lines.iter().map(|l| l.len() + 1).sum();
        assert!(
            consumed as u64 <= MAX_TAIL_BYTES,
            "read {consumed} bytes, window is {MAX_TAIL_BYTES}"
        );

        // The 500-line cap.
        let capped = tail_lines(&path, "web.log.2026-10-08", None, MAX_LINES).unwrap();
        assert!(capped.lines.len() <= MAX_LINES);
        assert_eq!(capped.lines.last().unwrap(), &last);
        assert!(capped.truncated);
    }

    #[test]
    fn log_tail_truncates_long_lines_on_a_char_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web.log.2026-10-08");
        // 10 KiB of 2-byte chars.
        let long = "é".repeat(5 * 1024);
        write(&path, &format!("{long}\nshort\n"));
        let r = tail_lines(&path, "web.log.2026-10-08", None, MAX_LINES).unwrap();
        assert_eq!(r.lines.len(), 2);
        assert!(r.lines[0].len() <= MAX_LINE_BYTES);
        assert!(r.lines[0].len() > MAX_LINE_BYTES - 4);
        assert_eq!(r.lines[1], "short");

        // 3-byte chars: 4096 is not a multiple of 3, so the cut backs off to 4095.
        let euro = "€".repeat(5000);
        let cut = truncate_line(&euro, MAX_LINE_BYTES);
        assert_eq!(cut.len(), 4095);
        assert!(euro.starts_with(&cut));
        // Short input is returned unchanged.
        assert_eq!(truncate_line("abc", MAX_LINE_BYTES), "abc");
    }

    #[tokio::test]
    async fn log_tail_clamps_max_lines() {
        let dir = tempfile::tempdir().unwrap();
        let _g = home(&dir);
        let logs = logs_dir(&dir);
        let mut body = String::new();
        for i in 0..600 {
            body.push_str(&format!("row-{i}\n"));
        }
        write(&logs.join("web.log.2026-10-08"), &body);

        let one = read_log_tail(LogSource::Server, 0, None).await.unwrap();
        assert_eq!(server_texts(&one), vec!["row-599"]);

        let many = read_log_tail(LogSource::Server, 9999, None).await.unwrap();
        assert_eq!(many.lines.len(), MAX_LINES);
        assert_eq!(server_texts(&many).last().unwrap(), "row-599");
        assert!(many.truncated);
    }

    #[tokio::test]
    async fn log_tail_missing_file_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let _g = home(&dir);
        // No logs/ directory at all.
        let page = read_log_tail(LogSource::Server, 100, None).await.unwrap();
        assert!(page.lines.is_empty());
        assert!(page.cursor.is_none());
        assert!(!page.reset);
        assert!(!page.truncated);
    }

    #[test]
    fn log_tail_server_level_classification() {
        let level = |s: &str| match classify_server_line(s.to_string()) {
            LogLine::Server { level, .. } => level,
            other => panic!("expected Server, got {other:?}"),
        };
        assert_eq!(
            level("2026-10-08T01:02:03Z ERROR iron_hermes_ui: boom"),
            Some("ERROR".to_string())
        );
        assert_eq!(
            level("2026-10-08T01:02:03Z  WARN iron_hermes_ui: hmm"),
            Some("WARN".to_string())
        );
        assert_eq!(
            level("2026-10-08T01:02:03Z INFO started"),
            Some("INFO".to_string())
        );
        assert_eq!(level("just some text"), None);
    }

    // ---------------------------------------------------------------
    // Task 2: audit source, incremental cursor / reset, symlink refusal.
    // ---------------------------------------------------------------

    fn audit_row(kind: &str, tool: &str, resolution: &str) -> String {
        serde_json::json!({
            "ts": "2026-10-08T01:02:03Z",
            "kind": kind,
            "tool": tool,
            "server": "srv",
            "session": "s-1",
            "surface": "web",
            "chat_id": "c-1",
            "reason": "guardrail",
            "decision": "approved",
            "resolution": resolution,
            "args": "{\"x\":1}",
        })
        .to_string()
    }

    #[tokio::test]
    async fn log_tail_audit_parses_rows_and_keeps_malformed_lines_raw() {
        let dir = tempfile::tempdir().unwrap();
        let _g = home(&dir);
        let logs = logs_dir(&dir);
        let body = format!(
            "{}\n{{not json\n{}\n",
            audit_row("mcp_mutation", "srv__tool", "operator"),
            audit_row("yolo_bypass_rollup", "terminal", "rollup"),
        );
        write(&logs.join("audit.jsonl"), &body);

        let page = read_log_tail(LogSource::Audit, 100, None).await.unwrap();
        assert_eq!(page.lines.len(), 3);
        match &page.lines[0] {
            LogLine::Audit(row) => {
                assert_eq!(row.kind, "mcp_mutation");
                assert_eq!(row.tool, "srv__tool");
                assert_eq!(row.server.as_deref(), Some("srv"));
                assert_eq!(row.session, "s-1");
                assert_eq!(row.decision, "approved");
                assert_eq!(row.args, "{\"x\":1}");
            }
            other => panic!("expected Audit, got {other:?}"),
        }
        assert_eq!(
            page.lines[1],
            LogLine::Raw {
                text: "{not json".to_string()
            }
        );
        match &page.lines[2] {
            LogLine::Audit(row) => {
                assert_eq!(row.kind, "yolo_bypass_rollup");
                assert_eq!(row.resolution, "rollup");
            }
            other => panic!("expected Audit, got {other:?}"),
        }
        assert_eq!(page.cursor.unwrap().file_key, "audit.jsonl");
    }

    #[tokio::test]
    async fn log_tail_audit_tolerates_missing_optional_fields() {
        let dir = tempfile::tempdir().unwrap();
        let _g = home(&dir);
        let logs = logs_dir(&dir);
        write(
            &logs.join("audit.jsonl"),
            "{\"ts\":\"t\",\"kind\":\"shell_command\",\"tool\":\"ls\"}\n[1,2]\n",
        );

        let page = read_log_tail(LogSource::Audit, 100, None).await.unwrap();
        match &page.lines[0] {
            LogLine::Audit(row) => {
                assert_eq!(row.kind, "shell_command");
                assert_eq!(row.server, None);
                assert_eq!(row.args, "");
                assert_eq!(row.session, "");
            }
            other => panic!("expected Audit, got {other:?}"),
        }
        // A JSON value that is not an object is kept raw, not dropped.
        assert_eq!(
            page.lines[1],
            LogLine::Raw {
                text: "[1,2]".to_string()
            }
        );
    }

    #[tokio::test]
    async fn log_tail_incremental_cursor_returns_only_appended_lines() {
        let dir = tempfile::tempdir().unwrap();
        let _g = home(&dir);
        let logs = logs_dir(&dir);
        let file = logs.join("web.log.2026-10-08");
        write(&file, "a\nb\n");

        let first = read_log_tail(LogSource::Server, 100, None).await.unwrap();
        assert_eq!(server_texts(&first), vec!["a", "b"]);
        let cursor = first.cursor.clone().unwrap();

        // Nothing appended: zero lines, cursor unchanged, no reset.
        let idle = read_log_tail(LogSource::Server, 100, Some(cursor.clone()))
            .await
            .unwrap();
        assert!(idle.lines.is_empty());
        assert_eq!(idle.cursor.as_ref(), Some(&cursor));
        assert!(!idle.reset);

        use std::io::Write;
        let mut f = fs::OpenOptions::new().append(true).open(&file).unwrap();
        f.write_all(b"c\nd\n").unwrap();
        drop(f);

        let second = read_log_tail(LogSource::Server, 100, Some(cursor))
            .await
            .unwrap();
        assert_eq!(server_texts(&second), vec!["c", "d"]);
        assert!(!second.reset);
        assert_eq!(second.cursor.unwrap().offset, 8);
    }

    #[tokio::test]
    async fn log_tail_resets_when_file_shrinks() {
        let dir = tempfile::tempdir().unwrap();
        let _g = home(&dir);
        let logs = logs_dir(&dir);
        write(&logs.join("web.log.2026-10-08"), "x\ny\n");
        let stale = LogCursor {
            file_key: "web.log.2026-10-08".to_string(),
            offset: 10_000,
        };

        let page = read_log_tail(LogSource::Server, 100, Some(stale))
            .await
            .unwrap();
        assert!(page.reset);
        assert_eq!(server_texts(&page), vec!["x", "y"]);
        assert_eq!(page.cursor.unwrap().offset, 4);
    }

    #[tokio::test]
    async fn log_tail_resets_on_new_daily_file() {
        let dir = tempfile::tempdir().unwrap();
        let _g = home(&dir);
        let logs = logs_dir(&dir);
        write(&logs.join("web.log.2026-10-07"), "yesterday\n");
        write(&logs.join("web.log.2026-10-08"), "today1\ntoday2\n");
        let yesterday = LogCursor {
            file_key: "web.log.2026-10-07".to_string(),
            offset: 10,
        };

        let page = read_log_tail(LogSource::Server, 100, Some(yesterday))
            .await
            .unwrap();
        assert!(page.reset);
        assert_eq!(server_texts(&page), vec!["today1", "today2"]);
        assert_eq!(page.cursor.unwrap().file_key, "web.log.2026-10-08");
    }

    #[test]
    fn log_tail_incremental_gap_larger_than_window_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web.log.2026-10-08");
        // ~1.5 MiB of 12-byte numbered lines.
        let total_lines = 1536 * 1024 / 12;
        let mut body = String::new();
        for i in 0..total_lines {
            body.push_str(&format!("line-{i:06}\n"));
        }
        write(&path, &body);
        let len = body.len() as u64;
        let cursor = LogCursor {
            file_key: "web.log.2026-10-08".to_string(),
            offset: len - 1024 * 1024,
        };

        let r = tail_lines(&path, "web.log.2026-10-08", Some(&cursor), usize::MAX).unwrap();
        assert!(!r.reset, "same file, valid offset: not a reset");
        assert!(r.truncated, "the gap exceeded the window");
        assert_eq!(r.end, len);
        let first = r.lines.first().unwrap();
        assert_eq!(first.len(), 11, "partial first line dropped, got {first:?}");
        let consumed: usize = r.lines.iter().map(|l| l.len() + 1).sum();
        assert!(consumed as u64 <= MAX_TAIL_BYTES);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn log_tail_refuses_symlinked_log() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let _g = home(&dir);
        let logs = logs_dir(&dir);
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        write(&secret, "TOP-SECRET-CONTENT\n");

        // A newer, symlinked web.log.* is never selected.
        write(&logs.join("web.log.2026-10-08"), "real\n");
        symlink(&secret, logs.join("web.log.2026-10-09")).unwrap();
        let page = read_log_tail(LogSource::Server, 100, None).await.unwrap();
        assert_eq!(server_texts(&page), vec!["real"]);
        assert_eq!(page.cursor.unwrap().file_key, "web.log.2026-10-08");

        // A symlinked audit.jsonl is refused with a path-free error.
        symlink(&secret, logs.join("audit.jsonl")).unwrap();
        let err = read_log_tail(LogSource::Audit, 100, None)
            .await
            .expect_err("symlinked audit.jsonl must be refused");
        let msg = err.to_string();
        assert!(!msg.contains("TOP-SECRET"), "target contents leaked: {msg}");
        assert!(
            !msg.contains(dir.path().to_str().unwrap()),
            "absolute path leaked: {msg}"
        );
        assert!(!msg.contains(outside.path().to_str().unwrap()));
    }

    /// WR-05: the open itself refuses a symlink. This calls the opener
    /// directly, i.e. it simulates the race where a regular file is swapped
    /// for a symlink AFTER the `symlink_metadata` check passed: `File::open`
    /// would follow the link and `fstat` the target (a regular file), so only
    /// `O_NOFOLLOW` stops it.
    #[cfg(unix)]
    #[test]
    fn open_regular_no_follow_refuses_a_symlink_swapped_in() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        write(&target, "TOP-SECRET-CONTENT\n");
        let log = dir.path().join("web.log.2026-10-09");

        // Before the swap: a regular file opens and reports its length.
        write(&log, "real\n");
        let (_, len) = open_regular_no_follow(&log).expect("a regular file opens");
        assert_eq!(len, 5);

        // After the swap: the symlink is refused even though its target is a
        // regular file, and tail_lines surfaces the same refusal.
        fs::remove_file(&log).unwrap();
        symlink(&target, &log).unwrap();
        assert!(
            fs::File::open(&log).unwrap().metadata().unwrap().is_file(),
            "premise: a plain open follows the link and fstat sees a regular file"
        );
        let err = open_regular_no_follow(&log).expect_err("a symlink must be refused");
        assert!(!err.to_string().contains("TOP-SECRET"));
        assert!(tail_lines(&log, "web.log.2026-10-09", None, 10).is_err());
    }

    /// WR-05: a FIFO swapped in neither blocks the open nor is read.
    #[cfg(unix)]
    #[test]
    fn open_regular_no_follow_refuses_a_fifo_without_blocking() {
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("audit.jsonl");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c_path` is a valid NUL-terminated path for the call.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        assert!(open_regular_no_follow(&fifo).is_err());
    }
}

//! REPL history persistence for the tui_rata backend (Phase 22.4 D-06/D-07/D-08).
//!
//! Implements the Phase 22.3 D-08 contract:
//! - File path: `$HERMES_HOME/repl_history`
//! - Format: rustyline plain-text, one entry per LF-delimited line
//! - Cap: 1000 entries (D-07)
//! - Dedupe: consecutive duplicates collapsed on push (HistoryDuplicates::Prev semantics)
//! - Multi-line entries: newlines encoded as U+001F (unit-separator) on disk;
//!   decoded back to `'\n'` on load. Backward-compatible with single-line
//!   rustyline entries (no `\u{1F}` → entry loaded with zero newlines).
//!
//! This module does NOT depend on rustyline — tui_rata uses tui-textarea
//! for input (D-05), so we hand-roll the minimal cursor + codec that D-06
//! arrow-key recall needs.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;

/// U+001F Unit Separator — ASCII C0 control character, guaranteed not to
/// appear in user-typed input.
const UNIT_SEP: char = '\u{1F}';

/// Default maximum number of entries retained on disk + in memory (D-07).
pub const DEFAULT_MAX: usize = 1000;

pub struct ReplHistory {
    entries: Vec<String>,
    cursor: Option<usize>,
    max: usize,
    dirty: bool,
}

impl ReplHistory {
    pub fn new(max: usize) -> Self {
        Self { entries: Vec::new(), cursor: None, max, dirty: false }
    }

    pub fn with_default_max() -> Self {
        Self::new(DEFAULT_MAX)
    }

    /// Load entries from a rustyline-compatible history file at `path`.
    /// Decodes `\u{1F}` → `'\n'` per entry. Missing file is not an error —
    /// returns an empty history.
    pub fn load(path: &Path, max: usize) -> std::io::Result<Self> {
        let mut hist = Self::new(max);
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(hist),
            Err(e) => return Err(e),
        };
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line?;
            if line.is_empty() { continue; }
            let decoded = decode_entry(&line);
            hist.entries.push(decoded);
        }
        // Apply cap AFTER full load (oldest entries dropped from front).
        if hist.entries.len() > hist.max {
            let drop_count = hist.entries.len() - hist.max;
            hist.entries.drain(0..drop_count);
        }
        hist.dirty = false;
        Ok(hist)
    }

    /// Persist to disk. Encodes `'\n'` → `\u{1F}` per entry.
    /// Creates parent directory if missing.
    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(path)?;
        for entry in &self.entries {
            writeln!(file, "{}", encode_entry(entry))?;
        }
        self.dirty = false;
        Ok(())
    }

    /// Append a new entry with dedupe-consecutive + cap enforcement.
    pub fn push(&mut self, entry: String) {
        if entry.is_empty() { return; }
        if let Some(last) = self.entries.last() {
            if *last == entry { return; }
        }
        self.entries.push(entry);
        if self.entries.len() > self.max {
            self.entries.remove(0);
        }
        self.dirty = true;
        self.cursor = None;
    }

    /// Recall: step backward (older). Called on KeyCode::Up per D-06.
    pub fn prev(&mut self) -> Option<&str> {
        if self.entries.is_empty() { return None; }
        let next_cursor = match self.cursor {
            None => self.entries.len().saturating_sub(1),
            Some(0) => 0, // stay at oldest
            Some(n) => n - 1,
        };
        self.cursor = Some(next_cursor);
        self.entries.get(next_cursor).map(String::as_str)
    }

    /// Recall: step forward (newer). Called on KeyCode::Down per D-06.
    /// Returns None when cursor advances past the newest entry (back to prompt).
    pub fn next(&mut self) -> Option<&str> {
        let current = self.cursor?;
        let new_cursor = current + 1;
        if new_cursor >= self.entries.len() {
            self.cursor = None;
            return None;
        }
        self.cursor = Some(new_cursor);
        self.entries.get(new_cursor).map(String::as_str)
    }

    pub fn reset_cursor(&mut self) { self.cursor = None; }

    pub fn len(&self) -> usize { self.entries.len() }

    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    pub fn is_dirty(&self) -> bool { self.dirty }
}

fn encode_entry(entry: &str) -> String {
    entry.replace('\n', &UNIT_SEP.to_string())
}

fn decode_entry(line: &str) -> String {
    line.replace(UNIT_SEP, "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn unit_separator_roundtrip_preserves_newlines() {
        let multi = "line 1\nline 2\nline 3";
        let encoded = encode_entry(multi);
        assert!(!encoded.contains('\n'), "encoded entry must not contain raw \\n");
        assert!(encoded.contains(UNIT_SEP), "encoded entry must contain \\u{{1F}}");
        let decoded = decode_entry(&encoded);
        assert_eq!(decoded, multi);
    }

    #[test]
    fn single_line_entry_has_no_unit_separator() {
        let single = "just one line";
        let encoded = encode_entry(single);
        assert_eq!(encoded, single, "single-line entries pass through unchanged (backward-compat)");
    }

    #[test]
    fn push_dedupes_consecutive() {
        let mut h = ReplHistory::new(1000);
        h.push("hello".to_string());
        h.push("hello".to_string()); // dedupe
        h.push("world".to_string());
        h.push("hello".to_string()); // not consecutive — re-added
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn push_enforces_cap() {
        let mut h = ReplHistory::new(3);
        h.push("a".into());
        h.push("b".into());
        h.push("c".into());
        h.push("d".into()); // evicts "a"
        assert_eq!(h.len(), 3);
        assert_eq!(h.prev(), Some("d"));
        h.reset_cursor();
        h.prev(); // cursor -> 2 = "d"
        h.prev(); // cursor -> 1 = "c"
        h.prev(); // cursor -> 0 = "b"
        assert_eq!(h.cursor, Some(0));
    }

    #[test]
    fn prev_next_cursor_walks_entries() {
        let mut h = ReplHistory::new(100);
        h.push("one".into());
        h.push("two".into());
        h.push("three".into());
        assert_eq!(h.prev(), Some("three"));
        assert_eq!(h.prev(), Some("two"));
        assert_eq!(h.prev(), Some("one"));
        assert_eq!(h.prev(), Some("one")); // stays at oldest
        assert_eq!(h.next(), Some("two"));
        assert_eq!(h.next(), Some("three"));
        assert_eq!(h.next(), None); // past newest -> back to prompt
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let path = std::env::temp_dir().join("tui_rata_history_nonexistent_test.txt");
        let _ = std::fs::remove_file(&path);
        let h = ReplHistory::load(&path, 1000).unwrap();
        assert!(h.is_empty());
    }

    #[test]
    fn save_then_load_roundtrip_including_multiline() {
        let path = std::env::temp_dir().join(format!("tui_rata_hist_rt_{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let mut h = ReplHistory::new(100);
        h.push("single line entry".into());
        h.push("multi\nline\nentry".into());
        h.push("another one".into());
        h.save(&path).unwrap();

        let reloaded = ReplHistory::load(&path, 100).unwrap();
        assert_eq!(reloaded.len(), 3);
        // Reload preserves order + decodes newlines
        let mut cursor_reloaded = reloaded;
        assert_eq!(cursor_reloaded.prev(), Some("another one"));
        assert_eq!(cursor_reloaded.prev(), Some("multi\nline\nentry"));
        assert_eq!(cursor_reloaded.prev(), Some("single line entry"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_legacy_rustyline_single_line_entries_unchanged() {
        // Simulate a pre-Phase-22.4 history file written by rustyline
        // (no \u{1F}). Should load as single-line entries.
        let path = std::env::temp_dir().join(format!("tui_rata_hist_legacy_{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "hello").unwrap();
        writeln!(f, "world").unwrap();
        drop(f);

        let h = ReplHistory::load(&path, 100).unwrap();
        assert_eq!(h.len(), 2);
        let mut h = h;
        assert_eq!(h.prev(), Some("world"));
        assert_eq!(h.prev(), Some("hello"));

        let _ = std::fs::remove_file(&path);
    }
}

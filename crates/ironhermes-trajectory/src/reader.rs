//! JSONL trajectory reader.
//!
//! Phase 25.3: minimal API consumed by Plan 11 (`hermes session export`)
//! and Phase 25.4 Curator's heuristic gate / summary builder.
//!
//! Per CONTEXT.md D-T-2: trajectories.jsonl is auxiliary, never read at session-load.
//! This reader is for OFFLINE consumption (export, curator, RL pipelines).

use crate::format::TrajectoryEntry;
use anyhow::{Context as _, Result};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Read-only handle for an existing `trajectories.jsonl` file.
///
/// Construct via `TrajectoryReader::open(path)`; consume via `read_all()`.
/// Missing files are treated as empty (Curator iterates this defensively).
pub struct TrajectoryReader {
    path: PathBuf,
}

impl TrajectoryReader {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Path of the trajectory file (may not exist yet).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read all entries from the JSONL file.
    ///
    /// Returns Ok(empty Vec) if the file does not exist (so Curator iteration
    /// is a no-op for sessions that produced no trajectories).
    /// Returns Err if a line fails to parse as TrajectoryEntry — loud failure
    /// surfaces format drift early.
    /// Skips blank / whitespace-only lines (defensive).
    pub fn read_all(&self) -> Result<Vec<TrajectoryEntry>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = std::fs::File::open(&self.path)
            .with_context(|| format!("open trajectory file {}", self.path.display()))?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();
        for (idx, line) in reader.lines().enumerate() {
            let line =
                line.with_context(|| format!("read line {} of {}", idx, self.path.display()))?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: TrajectoryEntry = serde_json::from_str(&line).with_context(|| {
                format!("parse trajectory line {} of {}", idx, self.path.display())
            })?;
            entries.push(entry);
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use crate::format::{ImpactLevel, TrajectoryEntry};
    use crate::reader::TrajectoryReader;
    use crate::writer::TrajectoryWriter;
    use tempfile::tempdir;

    #[test]
    fn read_all_missing_file_returns_empty_vec() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("does-not-exist.jsonl");
        let r = TrajectoryReader::open(&path);
        let entries = r.read_all().expect("missing file must NOT error");
        assert!(entries.is_empty());
    }

    #[test]
    fn read_all_empty_file_returns_empty_vec() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trajectories.jsonl");
        std::fs::write(&path, b"").unwrap();
        let r = TrajectoryReader::open(&path);
        let entries = r.read_all().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn read_all_skips_blank_lines() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trajectories.jsonl");
        // Two valid entries with a blank line in between
        let entry = TrajectoryEntry::success(
            "x",
            serde_json::json!({}),
            "ok",
            1,
            ImpactLevel::Read,
            0,
            "id1",
        );
        let json = serde_json::to_string(&entry).unwrap();
        let blob = format!("{json}\n\n{json}\n   \n");
        std::fs::write(&path, blob.as_bytes()).unwrap();
        let r = TrajectoryReader::open(&path);
        let entries = r.read_all().unwrap();
        assert_eq!(entries.len(), 2, "blank/whitespace lines must be skipped");
    }

    #[test]
    fn read_all_returns_entries_in_append_order() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trajectories.jsonl");
        {
            let mut w = TrajectoryWriter::open(&path).unwrap();
            for i in 0..5 {
                let e = TrajectoryEntry::success(
                    format!("t{i}"),
                    serde_json::json!({"i": i}),
                    "ok",
                    i as u64,
                    ImpactLevel::Read,
                    i,
                    format!("id{i}"),
                );
                w.append(&e).unwrap();
            }
        }
        let r = TrajectoryReader::open(&path);
        let entries = r.read_all().unwrap();
        assert_eq!(entries.len(), 5);
        for (i, e) in entries.iter().enumerate() {
            assert_eq!(e.tool_call_id, format!("id{i}"), "order preserved");
            assert_eq!(e.turn_index, i);
        }
    }

    #[test]
    fn read_all_errors_loudly_on_malformed_line() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trajectories.jsonl");
        std::fs::write(&path, b"this is not json\n").unwrap();
        let r = TrajectoryReader::open(&path);
        let res = r.read_all();
        assert!(res.is_err(), "malformed JSON must surface as Err");
        let msg = format!("{:#}", res.unwrap_err());
        assert!(
            msg.contains("parse trajectory line"),
            "error must name the line; got: {msg}"
        );
    }
}

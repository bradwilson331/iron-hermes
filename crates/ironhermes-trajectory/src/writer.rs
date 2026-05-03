//! Append-only JSONL trajectory writer with crash-safe fsync-per-line.
//!
//! Phase 25.3 D-T-2: per-tool-call records appended one line at a time;
//! `sync_data()` is called after EVERY write so a process kill between lines
//! cannot corrupt prior entries. `Drop for TrajectoryWriter` also calls
//! `sync_data().ok()` — without it, the final entry can be lost on panic
//! / Ctrl+C (RESEARCH.md Pitfall 3).
//!
//! Multi-process writers are NOT expected (RESEARCH.md A4): a single session_id
//! is owned by one surface at a time. O_APPEND POSIX semantics give line-level
//! atomicity for writes <= PIPE_BUF (4 KiB) which a typical TrajectoryEntry
//! comfortably fits within.

#[cfg(test)]
mod tests {
    use crate::format::{ImpactLevel, TrajectoryEntry};
    use crate::writer::TrajectoryWriter;
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    use std::path::Path;
    use tempfile::tempdir;

    fn sample_entry(call_id: &str, turn: usize) -> TrajectoryEntry {
        TrajectoryEntry::success(
            "write_file",
            serde_json::json!({"path": "/tmp/x"}),
            "wrote 0 bytes",
            42,
            ImpactLevel::Write,
            turn,
            call_id,
        )
    }

    fn read_lines(path: &Path) -> Vec<String> {
        let f = File::open(path).expect("reopen trajectory file");
        BufReader::new(f).lines().map(|l| l.unwrap()).collect()
    }

    #[test]
    fn open_creates_parent_directories() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("c").join("trajectories.jsonl");
        assert!(!path.parent().unwrap().exists());
        let _w = TrajectoryWriter::open(&path).expect("open with missing parent dirs");
        assert!(path.parent().unwrap().exists(), "parent dirs must be created");
        assert!(path.exists(), "trajectory file must be created");
    }

    #[test]
    fn append_writes_jsonl_line_with_newline() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trajectories.jsonl");
        let mut w = TrajectoryWriter::open(&path).unwrap();
        w.append(&sample_entry("t1", 0)).unwrap();
        // Read back via separate handle to verify on-disk state
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1, "exactly one line after one append");
        assert!(lines[0].contains("\"tool_call_id\":\"t1\""), "got: {:?}", lines[0]);
        // Verify no embedded newline INSIDE the JSON (the serializer's job, locked by Plan 1)
        assert!(!lines[0].contains('\n'));
    }

    #[test]
    fn append_persists_to_disk_after_each_call() {
        // The fsync-per-line invariant: after every append, a separate reader
        // can see the entry without needing to wait for buffer flushes.
        let dir = tempdir().unwrap();
        let path = dir.path().join("trajectories.jsonl");
        let mut w = TrajectoryWriter::open(&path).unwrap();
        for i in 0..5 {
            w.append(&sample_entry(&format!("t{i}"), i)).unwrap();
            let lines = read_lines(&path);
            assert_eq!(lines.len(), i + 1, "after {} appends, {} lines on disk", i + 1, i + 1);
        }
    }

    #[test]
    fn drop_flushes_final_entry() {
        // Pitfall 3 guard: Drop must call sync_data so the final append survives.
        // Simulate by appending then dropping the writer in an inner scope, then
        // reopening from cold and confirming the entry is on disk.
        let dir = tempdir().unwrap();
        let path = dir.path().join("trajectories.jsonl");
        {
            let mut w = TrajectoryWriter::open(&path).unwrap();
            w.append(&sample_entry("final", 99)).unwrap();
            // w drops here — Drop's sync_data().ok() runs
        }
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1, "drop must persist the entry");
        assert!(lines[0].contains("\"tool_call_id\":\"final\""));
    }

    #[test]
    fn reopen_appends_to_existing_file() {
        // O_APPEND semantics: closing and reopening preserves prior entries
        // and new writes go to the end.
        let dir = tempdir().unwrap();
        let path = dir.path().join("trajectories.jsonl");
        {
            let mut w = TrajectoryWriter::open(&path).unwrap();
            w.append(&sample_entry("first", 0)).unwrap();
        }
        {
            let mut w = TrajectoryWriter::open(&path).unwrap();
            w.append(&sample_entry("second", 1)).unwrap();
        }
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 2, "two appends across two writer lifetimes");
        assert!(lines[0].contains("\"tool_call_id\":\"first\""));
        assert!(lines[1].contains("\"tool_call_id\":\"second\""));
    }

    #[test]
    fn writer_path_accessor_returns_open_path() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trajectories.jsonl");
        let w = TrajectoryWriter::open(&path).unwrap();
        assert_eq!(w.path(), path.as_path());
    }

    #[test]
    fn open_fails_with_clear_error_when_parent_unwritable() {
        // Best-effort sanity test: try to open under a path that requires root.
        // On macOS/Linux /proc and /sys/firmware are typically readonly. Skip if
        // the platform allows the open (e.g., running as root in CI).
        let p = Path::new("/proc/this-is-not-a-real-trajectory-dir/trajectories.jsonl");
        if p.parent().map(|pp| pp.exists()).unwrap_or(false) {
            return; // skip if parent already exists and is writable
        }
        let res = TrajectoryWriter::open(p);
        // Either the create_dir_all fails (Err) OR the open fails (Err) — both acceptable.
        // Critical: NO panic, error message contains the path.
        if let Err(e) = res {
            let msg = format!("{:#}", e);
            assert!(msg.contains("trajectory") || msg.contains("/proc"),
                "error must reference the failed path/op; got: {msg}");
        }
    }
}

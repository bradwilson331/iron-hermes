//! End-to-end: append N entries via TrajectoryWriter, drop, read_all via TrajectoryReader.
//! Locks the wire-format compatibility between writer + reader within the same crate.

use ironhermes_trajectory::{ImpactLevel, TrajectoryEntry, TrajectoryReader, TrajectoryWriter};
use tempfile::tempdir;

#[test]
fn writer_reader_roundtrip_10_entries() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("session-id-x").join("trajectories.jsonl");
    {
        let mut w = TrajectoryWriter::open(&path).unwrap();
        for i in 0..10 {
            let impact = match i % 3 {
                0 => ImpactLevel::Read,
                1 => ImpactLevel::Write,
                _ => ImpactLevel::SystemChange,
            };
            let e = if i == 7 {
                TrajectoryEntry::failure(
                    "terminal",
                    serde_json::json!({"cmd": "false"}),
                    "exit 1",
                    1,
                    impact,
                    i,
                    format!("call-{i}"),
                )
            } else {
                TrajectoryEntry::success(
                    format!("tool-{i}"),
                    serde_json::json!({"arg": i}),
                    format!("result-{i}"),
                    (i + 1) as u64,
                    impact,
                    i,
                    format!("call-{i}"),
                )
            };
            w.append(&e).unwrap();
        }
    }
    let r = TrajectoryReader::open(&path);
    let entries = r.read_all().expect("read_all roundtrip");
    assert_eq!(entries.len(), 10);
    for (i, e) in entries.iter().enumerate() {
        assert_eq!(e.tool_call_id, format!("call-{i}"));
        assert_eq!(e.turn_index, i);
        if i == 7 {
            assert!(e.result.is_none());
            assert_eq!(e.error.as_deref(), Some("exit 1"));
        } else {
            assert_eq!(e.result.as_deref(), Some(format!("result-{i}").as_str()));
            assert!(e.error.is_none());
        }
    }
}

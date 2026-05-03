//! Golden-file format tests for `ironhermes-trajectory` JSONL output (Phase 25.3 D-T-1).
//!
//! These tests assert the on-disk wire format of `TrajectoryEntry` so downstream
//! consumers (Phase 25.4 Curator, RL pipelines, hermes session export-all dumps)
//! can rely on a stable schema. Update the expected substrings ONLY with an
//! intentional format change AND a corresponding migration plan for consumers.

use ironhermes_trajectory::format::{ImpactLevel, TrajectoryEntry};

#[test]
fn trajectory_entry_jsonl_format_matches_golden() {
    let entry = TrajectoryEntry {
        name: "write_file".to_string(),
        args: serde_json::json!({"path": "/tmp/test.txt", "content": "hello"}),
        result: Some("wrote 5 bytes".to_string()),
        error: None,
        duration_ms: 42,
        impact_level: ImpactLevel::Write,
        turn_index: 1,
        tool_call_id: "toolu_abc123".to_string(),
        ts: "2026-05-03T10:00:00+00:00".to_string(),
    };
    let line = serde_json::to_string(&entry).unwrap();

    // Required D-T-1 fields:
    assert!(line.contains("\"name\":\"write_file\""), "missing name; got: {line}");
    assert!(line.contains("\"impact_level\":\"write\""), "missing impact_level; got: {line}");
    assert!(line.contains("\"turn_index\":1"), "missing turn_index; got: {line}");
    assert!(line.contains("\"duration_ms\":42"), "missing duration_ms; got: {line}");
    assert!(line.contains("\"tool_call_id\":\"toolu_abc123\""), "missing tool_call_id; got: {line}");
    assert!(line.contains("\"ts\":\"2026-05-03T10:00:00+00:00\""), "missing ts; got: {line}");
    assert!(line.contains("\"args\":"), "missing args; got: {line}");
    assert!(line.contains("\"result\":\"wrote 5 bytes\""), "missing result; got: {line}");

    // Wire-shape invariants for Plan 4 writer:
    assert!(
        !line.ends_with('\n'),
        "serde_json::to_string must NOT add a trailing newline — the writer adds it; got: {line:?}"
    );
    assert!(
        !line.contains('\n'),
        "TrajectoryEntry JSONL line must be single-line (no embedded newlines); got: {line:?}"
    );
}

#[test]
fn impact_level_wire_strings_locked() {
    // Phase 25.4 Curator heuristic D-C-2 expects these literal strings.
    assert_eq!(serde_json::to_string(&ImpactLevel::Read).unwrap(), "\"read\"");
    assert_eq!(serde_json::to_string(&ImpactLevel::Write).unwrap(), "\"write\"");
    assert_eq!(serde_json::to_string(&ImpactLevel::SystemChange).unwrap(), "\"system_change\"");
}

#[test]
fn jsonl_lines_concatenate_cleanly() {
    // Multi-entry append simulation: each line ends in '\n' (writer's responsibility),
    // and every line parses independently. This locks the JSONL format Plan 4 will write.
    let entries = vec![
        TrajectoryEntry {
            name: "a".to_string(),
            args: serde_json::json!({}),
            result: Some("ok".to_string()),
            error: None,
            duration_ms: 1,
            impact_level: ImpactLevel::Read,
            turn_index: 0,
            tool_call_id: "t1".to_string(),
            ts: "2026-05-03T10:00:00+00:00".to_string(),
        },
        TrajectoryEntry {
            name: "b".to_string(),
            args: serde_json::json!({"x": 1}),
            result: None,
            error: Some("boom".to_string()),
            duration_ms: 2,
            impact_level: ImpactLevel::SystemChange,
            turn_index: 1,
            tool_call_id: "t2".to_string(),
            ts: "2026-05-03T10:00:01+00:00".to_string(),
        },
    ];
    let blob: String = entries.iter().map(|e| serde_json::to_string(e).unwrap() + "\n").collect();
    let lines: Vec<&str> = blob.split_terminator('\n').collect();
    assert_eq!(lines.len(), 2, "two entries -> two lines");
    for (i, l) in lines.iter().enumerate() {
        let parsed: TrajectoryEntry = serde_json::from_str(l)
            .unwrap_or_else(|e| panic!("line {i} must parse: err={e}, line={l:?}"));
        assert_eq!(parsed.tool_call_id, format!("t{}", i + 1));
    }
}

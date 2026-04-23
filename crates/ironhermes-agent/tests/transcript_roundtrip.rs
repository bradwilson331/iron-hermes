//! D-05 schema durability + E-08 no-panic-on-error test.

use chrono::Utc;
use ironhermes_agent::transcript::*;

#[test]
fn tool_call_round_trips() {
    let line = TranscriptLine::ToolCall {
        at: Utc::now(),
        tool: "exec".into(),
        args_preview: "ls -la".into(),
    };
    let s = serde_json::to_string(&line).unwrap();
    assert!(s.contains("\"event\":\"tool_call\""));
    let back: TranscriptLine = serde_json::from_str(&s).unwrap();
    assert_eq!(back, line);
}

#[test]
fn every_variant_round_trips_with_correct_tag() {
    let ts = Utc::now();
    let cases: Vec<(&str, TranscriptLine)> = vec![
        (
            "tool_call",
            TranscriptLine::ToolCall {
                at: ts,
                tool: "t".into(),
                args_preview: "p".into(),
            },
        ),
        (
            "tool_result",
            TranscriptLine::ToolResult {
                at: ts,
                tool: "t".into(),
                ok: true,
                content_preview: "ok".into(),
            },
        ),
        (
            "stream_delta",
            TranscriptLine::StreamDelta {
                at: ts,
                delta: "hello".into(),
            },
        ),
        (
            "done",
            TranscriptLine::Done {
                at: ts,
                final_response_preview: "bye".into(),
            },
        ),
        (
            "cancelled",
            TranscriptLine::Cancelled {
                at: ts,
                reason: "user interrupt".into(),
            },
        ),
    ];
    for (tag, line) in cases {
        let s = serde_json::to_string(&line).unwrap();
        assert!(
            s.contains(&format!("\"event\":\"{}\"", tag)),
            "variant {:?} must serialize with event tag '{}', got: {}",
            line,
            tag,
            s
        );
        let back: TranscriptLine = serde_json::from_str(&s).unwrap();
        assert_eq!(back, line);
    }
}

#[tokio::test]
async fn writer_append_to_read_only_dir_does_not_panic() {
    // Force `create_dir_all` to fail by making the parent a FILE, not a dir.
    let tmp = tempfile::tempdir().unwrap();
    let blocker = tmp.path().join("blocker");
    std::fs::write(&blocker, b"").unwrap();
    let bad_path = blocker.join("inner").join("x.jsonl");

    let writer = TranscriptWriter::open(&bad_path);
    writer.append(TranscriptLine::now_done("hi"));
    // Allow fire-and-forget task to run + log.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    // The assertion is implicit: no panic reached here.
}

#[tokio::test]
async fn writer_appends_three_lines_and_reads_back() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sub123.jsonl");
    let w = TranscriptWriter::open(&path);
    w.append(TranscriptLine::now_stream_delta("a"));
    w.append(TranscriptLine::now_stream_delta("b"));
    w.append(TranscriptLine::now_cancelled("user"));
    // Drain fire-and-forget.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let body = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 3);
    // Last line is cancelled (E-08 / D-07).
    let last: TranscriptLine = serde_json::from_str(lines[2]).unwrap();
    assert!(
        matches!(last, TranscriptLine::Cancelled { .. }),
        "E-08 / D-07 — cancellation marker MUST be the last line on cancel path"
    );
}

#[test]
fn transcript_path_shape() {
    let home = std::path::Path::new("/tmp/hermes");
    let p = transcript_path_for(home, "sess-abc", "sub-xyz");
    assert_eq!(
        p,
        std::path::PathBuf::from("/tmp/hermes/subagent-transcripts/sess-abc/sub-xyz.jsonl"),
        "D-05 path shape contract"
    );
}

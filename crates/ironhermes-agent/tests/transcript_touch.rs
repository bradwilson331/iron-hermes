//! Phase 22.3 Plan 02: integration test for TranscriptWriter::touch().
//! Closes UAT D-3 (alias→transcript race).
//! Locked by INV-22.3-05 (Plan 22.3-06 grep gate).

use ironhermes_agent::transcript::TranscriptWriter;
use std::fs;

#[tokio::test]
async fn transcript_file_exists_after_touch() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("sub_abc123.jsonl");
    let writer = TranscriptWriter::open(path.clone());
    writer.touch().await;
    assert!(
        fs::metadata(&path).is_ok(),
        "INV-22.3-05: transcript file must exist on disk after touch(), \
         before any append() write (so /agents logs can stat it the moment \
         the alias becomes queryable via SubagentRegistry::register)."
    );
}

#[tokio::test]
async fn touch_is_idempotent_does_not_truncate() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("sub_def456.jsonl");
    let writer = TranscriptWriter::open(path.clone());
    writer.touch().await;
    fs::write(&path, b"existing-content\n").expect("write");
    writer.touch().await; // second touch should NOT truncate
    let content = fs::read(&path).expect("read");
    assert_eq!(
        content, b"existing-content\n",
        "touch() must use .append(true) not .truncate(true) — second touch destroyed content"
    );
}

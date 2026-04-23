//! E-08 / AI-SPEC Pitfall 3: transcript writer must be fire-and-forget
//! with swallowed errors. Never `.unwrap()` or `.expect(...)` on a write.
//!
//! This test is GREEN in Wave 0 because `transcript.rs` does not exist yet
//! (`include_str!` would fail-compile); it becomes MEANINGFUL in Plan 04 Task
//! 4-02. We use a Path-based check for Wave 0 so CI stays green.

use std::path::Path;

#[test]
fn transcript_writer_has_no_unwrap_when_present() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("transcript.rs");
    if !path.exists() {
        eprintln!("E-08 pending: transcript.rs arrives in Plan 04 (Wave 1).");
        return;
    }
    let src = std::fs::read_to_string(&path).expect("read transcript.rs");
    assert!(
        !src.contains(".unwrap()"),
        "E-08 / Pitfall 3: transcript.rs MUST NOT contain .unwrap() — fire-and-forget writes log a tracing::warn and swallow."
    );
    assert!(
        !src.contains(".expect("),
        "E-08: transcript.rs MUST NOT use .expect(...) on writes either."
    );
}

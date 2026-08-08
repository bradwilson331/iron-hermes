// Phase 39.2 Plan 04 Task 2: automated tests for ironhermes-blackbox.
//
// Coverage:
//   - redact_removes_card_number
//   - redact_removes_email
//   - redact_removes_api_key_pattern
//   - redact_preserves_structure
//   - argument_hash_is_stable          (same content, different key order → same hash)
//   - argument_hash_differs_for_different_args
//   - argument_hash_is_hex_string
//   - recorder_appends_valid_jsonl
//   - recorder_pii_redacted_in_jsonl

use ironhermes_blackbox::{
    BlackBoxEvent, BlackBoxRecorder, Stage, argument_hash, redact, redact_str,
};
use serde_json::{Value, json};
use uuid::Uuid;

// ─── redact / redact_str tests ───────────────────────────────────────────────

#[test]
fn redact_removes_card_number() {
    let v = json!({ "card": "4111111111111111" });
    let out = redact(v);
    let card = out["card"].as_str().unwrap();
    assert_eq!(card, "[REDACTED]", "16-digit card number must be redacted");
}

#[test]
fn redact_removes_email() {
    let s = "Contact us at support@example.com for help";
    let out = redact_str(s);
    assert!(
        !out.contains("support@example.com"),
        "email address must be redacted; got: {out}"
    );
    assert!(
        out.contains("[REDACTED]"),
        "placeholder must appear; got: {out}"
    );
}

#[test]
fn redact_removes_api_key_pattern() {
    // Matches the regex: `(?i)(api[_-]?key|token|secret|password)\s*[:=]\s*\S+`
    let s = "Authorization api_key=sk-test-abc123";
    let out = redact_str(s);
    assert!(
        !out.contains("sk-test-abc123"),
        "API key value must be redacted; got: {out}"
    );
    assert!(
        out.contains("[REDACTED]"),
        "placeholder must appear; got: {out}"
    );
}

#[test]
fn redact_preserves_structure() {
    // Non-sensitive fields must be preserved unchanged.
    let v = json!({
        "tool_name": "search",
        "latency_ms": 42,
        "success": true,
        "nested": { "count": 7 }
    });
    let out = redact(v);
    assert_eq!(out["tool_name"].as_str().unwrap(), "search");
    assert_eq!(out["latency_ms"].as_u64().unwrap(), 42);
    assert!(out["success"].as_bool().unwrap());
    assert_eq!(out["nested"]["count"].as_u64().unwrap(), 7);
}

// ─── argument_hash tests ─────────────────────────────────────────────────────

#[test]
fn argument_hash_is_stable() {
    // The same logical JSON object, different insertion order, must hash identically.
    let a = json!({ "b": 2, "a": 1 });
    let b = json!({ "a": 1, "b": 2 });
    assert_eq!(
        argument_hash(&a),
        argument_hash(&b),
        "key-order-independent hashing must produce identical hashes"
    );
}

#[test]
fn argument_hash_differs_for_different_args() {
    let a = json!({ "query": "iron hermes" });
    let b = json!({ "query": "something else" });
    assert_ne!(
        argument_hash(&a),
        argument_hash(&b),
        "different argument values must produce different hashes"
    );
}

#[test]
fn argument_hash_is_hex_string() {
    let v = json!({ "tool": "search", "k": 5 });
    let hash = argument_hash(&v);
    // SHA-256 hex is exactly 64 lowercase hex characters.
    assert_eq!(
        hash.len(),
        64,
        "SHA-256 hex must be 64 chars; got len={}",
        hash.len()
    );
    assert!(
        hash.chars().all(|c| c.is_ascii_hexdigit()),
        "hash must be lowercase hex; got: {hash}"
    );
}

// ─── recorder integration tests ──────────────────────────────────────────────

/// Helper: build a `BlackBoxRecorder` writing to a temp dir, emit one event,
/// wait for the writer task to flush, then read back the file.
async fn record_and_read(event: BlackBoxEvent) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();

    let recorder = BlackBoxRecorder::new("hermes-test".to_string(), home.clone());
    recorder.try_record(event);

    // Give the background task time to flush the event. The writer flushes after
    // every event, so 200 ms is more than sufficient in CI.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // The file is named blackbox-YYYY-MM-DD.jsonl in home_dir/logs.
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let path = home.join("logs").join(format!("blackbox-{today}.jsonl"));
    std::fs::read_to_string(&path).unwrap_or_default()
}

#[tokio::test]
async fn recorder_appends_valid_jsonl() {
    let run_id = Uuid::new_v4();
    let event = BlackBoxRecorder::make_event(
        run_id,
        Stage::Tool,
        "tool_dispatched",
        "hermes-test",
        json!({ "tool_name": "search", "latency_ms": 0 }),
    );

    let content = record_and_read(event).await;

    assert!(!content.is_empty(), "JSONL file must not be empty");

    // Every non-empty line must be valid JSON.
    for line in content.lines().filter(|l| !l.trim().is_empty()) {
        let parsed: Result<Value, _> = serde_json::from_str(line);
        assert!(parsed.is_ok(), "line must be valid JSON; got: {line}");
    }

    // The emitted event must carry the correct run_id and event_name.
    let first: Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();
    assert_eq!(first["run_id"].as_str().unwrap(), run_id.to_string());
    assert_eq!(first["event_name"].as_str().unwrap(), "tool_dispatched");
    assert_eq!(first["workflow_version"].as_str().unwrap(), "hermes-test");
    assert_eq!(first["stage"].as_str().unwrap(), "tool");
}

#[tokio::test]
async fn recorder_pii_redacted_in_jsonl() {
    let run_id = Uuid::new_v4();
    let event = BlackBoxRecorder::make_event(
        run_id,
        Stage::Input,
        "turn_started",
        "hermes-test",
        json!({
            "user_input": "my card is 4111111111111111 and email admin@secret.org",
            "session_id": "sess-abc"
        }),
    );

    let content = record_and_read(event).await;

    assert!(
        !content.is_empty(),
        "JSONL file must not be empty after PII event"
    );

    // Raw PII must NOT appear on disk.
    assert!(
        !content.contains("4111111111111111"),
        "card number must be redacted in JSONL; content:\n{content}"
    );
    assert!(
        !content.contains("admin@secret.org"),
        "email must be redacted in JSONL; content:\n{content}"
    );

    // The [REDACTED] placeholder must appear.
    assert!(
        content.contains("[REDACTED]"),
        "redaction placeholder must appear in JSONL; content:\n{content}"
    );

    // Non-sensitive field must survive.
    assert!(
        content.contains("sess-abc"),
        "non-sensitive session_id must be preserved; content:\n{content}"
    );
}

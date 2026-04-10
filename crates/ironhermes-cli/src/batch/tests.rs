use super::types::*;
use super::checkpoint::*;
use super::sharegpt::*;

#[test]
fn test_prompt_hash_deterministic() {
    let h1 = prompt_hash("hello world");
    let h2 = prompt_hash("hello world");
    assert_eq!(h1, h2);
    assert_eq!(h1.len(), 64); // SHA-256 hex is 64 chars
}

#[test]
fn test_prompt_hash_different_inputs() {
    let h1 = prompt_hash("prompt one");
    let h2 = prompt_hash("prompt two");
    assert_ne!(h1, h2);
}

#[test]
fn test_sharegpt_user_message() {
    use ironhermes_core::ChatMessage;
    let msgs = vec![ChatMessage::user("Hello")];
    let turns = messages_to_sharegpt(&msgs);
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].from, "human");
    assert_eq!(turns[0].value, "Hello");
}

#[test]
fn test_sharegpt_skips_system() {
    use ironhermes_core::ChatMessage;
    let msgs = vec![ChatMessage::system("You are helpful"), ChatMessage::user("Hi")];
    let turns = messages_to_sharegpt(&msgs);
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].from, "human");
}

#[test]
fn test_trajectory_line_serializes_to_json() {
    let traj = TrajectoryLine {
        id: "abc123".to_string(),
        model: "gpt-4".to_string(),
        timestamp: "2026-04-10T00:00:00Z".to_string(),
        usage: UsageInfo { prompt_tokens: 100, completion_tokens: 50 },
        turns: 3,
        quality: QualityResult { passed: true, reasons: vec![] },
        conversations: vec![
            ShareGptTurn { from: "human".to_string(), value: "Hello".to_string() },
            ShareGptTurn { from: "gpt".to_string(), value: "Hi there".to_string() },
        ],
        rejection_reason: None,
    };
    let json = serde_json::to_string(&traj).unwrap();
    assert!(json.contains("\"conversations\""));
    assert!(json.contains("\"human\""));
    assert!(!json.contains("rejection_reason")); // skip_serializing_if = None
}

#[test]
fn test_checkpoint_entry_roundtrip() {
    let entry = CheckpointEntry {
        status: "completed".to_string(),
        timestamp: "2026-04-10T00:00:00Z".to_string(),
    };
    let json = serde_json::to_string(&entry).unwrap();
    let parsed: CheckpointEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.status, "completed");
}

#[test]
fn test_batch_entry_minimal_parse() {
    let json = r#"{"prompt": "What is Rust?"}"#;
    let entry: BatchEntry = serde_json::from_str(json).unwrap();
    assert_eq!(entry.prompt, "What is Rust?");
    assert!(entry.system.is_none());
    assert!(entry.tools.is_none());
}

#[test]
fn test_batch_entry_with_optional_fields() {
    let json = r#"{"prompt": "Hello", "system": "Be concise", "tools": ["web_read"]}"#;
    let entry: BatchEntry = serde_json::from_str(json).unwrap();
    assert_eq!(entry.system.unwrap(), "Be concise");
    assert_eq!(entry.tools.unwrap(), vec!["web_read"]);
}

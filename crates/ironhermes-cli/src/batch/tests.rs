// Phase 10 Plan 04 + Plan 02 tests — filters + runner integration
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

// =============================================================================
// Filter tests
// =============================================================================

use ironhermes_agent::{AgentResult, AggregatedUsage};
use ironhermes_agent::agent_loop::StopReason;
use ironhermes_core::{ChatMessage, FunctionCall, ToolCall};
use ironhermes_tools::ToolRegistry;
use super::filters::*;

/// Build a minimal AgentResult for filter testing.
fn mock_agent_result(messages: Vec<ChatMessage>, final_response: Option<String>) -> AgentResult {
    AgentResult {
        messages,
        appended: Vec::new(),
        turns_used: 1,
        finished_naturally: true,
        final_response,
        total_usage: AggregatedUsage::default(),
        compression_count_after: 0,
        // Plan 21.7-05: new required field; batch filter tests are
        // structural — a natural-completion stop reason is correct.
        stop_reason: StopReason::Natural,
    }
}

/// Build a minimal ToolRegistry with a single named tool.
fn registry_with(tool_name: &'static str) -> ToolRegistry {
    use async_trait::async_trait;
    use ironhermes_core::ToolSchema;
    use ironhermes_tools::Tool;

    struct MockTool { name: &'static str }

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str { self.name }
        fn toolset(&self) -> &str { "test" }
        fn description(&self) -> &str { "mock" }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(self.name, "mock", serde_json::json!({"type":"object","properties":{}}))
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
    }

    let mut r = ToolRegistry::new();
    r.register(Box::new(MockTool { name: tool_name }));
    r
}

fn make_tool_call(name: &str) -> ToolCall {
    ToolCall {
        id: "tc1".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: name.to_string(),
            arguments: "{}".to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// filter_hallucinated_tools
// ---------------------------------------------------------------------------

#[test]
fn test_filter_hallucinated_tools_detects_unknown() {
    let registry = registry_with("web_read");
    let msg = ChatMessage::assistant_tool_calls(vec![make_tool_call("fake_tool")]);
    let result = mock_agent_result(vec![msg], None);
    let rejection = filter_hallucinated_tools(&result, &registry);
    assert_eq!(rejection, Some("hallucinated_tool:fake_tool".to_string()));
}

#[test]
fn test_filter_hallucinated_tools_passes_known() {
    let registry = registry_with("web_read");
    let msg = ChatMessage::assistant_tool_calls(vec![make_tool_call("web_read")]);
    let result = mock_agent_result(vec![msg], None);
    let rejection = filter_hallucinated_tools(&result, &registry);
    assert_eq!(rejection, None);
}

// ---------------------------------------------------------------------------
// filter_no_reasoning
// ---------------------------------------------------------------------------

#[test]
fn test_filter_no_reasoning_rejects_empty() {
    let result = mock_agent_result(vec![], None);
    assert_eq!(filter_no_reasoning(&result), Some("no_reasoning_steps".to_string()));
}

#[test]
fn test_filter_no_reasoning_rejects_empty_response() {
    let result = mock_agent_result(vec![ChatMessage::user("hi")], Some("".to_string()));
    assert_eq!(filter_no_reasoning(&result), Some("no_reasoning_steps".to_string()));
}

#[test]
fn test_filter_no_reasoning_passes_with_tools() {
    let msg = ChatMessage::assistant_tool_calls(vec![make_tool_call("web_read")]);
    let result = mock_agent_result(vec![msg], None);
    assert_eq!(filter_no_reasoning(&result), None);
}

#[test]
fn test_filter_no_reasoning_rejects_text_only() {
    // UAT gap: text-only responses without tool calls must be rejected
    let result = mock_agent_result(vec![], Some("This is a substantive response".to_string()));
    assert_eq!(filter_no_reasoning(&result), Some("no_reasoning_steps".to_string()));
}

// ---------------------------------------------------------------------------
// filter_error_only
// ---------------------------------------------------------------------------

#[test]
fn test_filter_error_only_rejects_all_errors() {
    let msgs = vec![
        ChatMessage::tool_result("tc1", "Error: something went wrong"),
        ChatMessage::tool_result("tc2", "Error: another failure"),
    ];
    let result = mock_agent_result(msgs, None);
    assert_eq!(filter_error_only(&result), Some("error_only_trajectory".to_string()));
}

#[test]
fn test_filter_error_only_passes_mixed() {
    let msgs = vec![
        ChatMessage::tool_result("tc1", "Error: something went wrong"),
        ChatMessage::tool_result("tc2", "Success: file read successfully"),
    ];
    let result = mock_agent_result(msgs, None);
    assert_eq!(filter_error_only(&result), None);
}

#[test]
fn test_filter_error_only_passes_no_tool_results() {
    // No tool result messages — not an error-only trajectory
    let result = mock_agent_result(vec![ChatMessage::user("hi")], None);
    assert_eq!(filter_error_only(&result), None);
}

// ---------------------------------------------------------------------------
// filter_secrets_in_output
// ---------------------------------------------------------------------------

#[test]
fn test_filter_secrets_detects_api_key() {
    let msgs = vec![ChatMessage::tool_result(
        "tc1",
        "Response: api_key=sk-live-abcdefghijklmnopqrstuvwxyz1234",
    )];
    let result = mock_agent_result(msgs, None);
    assert_eq!(filter_secrets_in_output(&result), Some("secrets_in_output".to_string()));
}

#[test]
fn test_filter_secrets_detects_bearer_jwt() {
    let msgs = vec![ChatMessage::tool_result(
        "tc1",
        "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.abc.signature",
    )];
    let result = mock_agent_result(msgs, None);
    assert_eq!(filter_secrets_in_output(&result), Some("secrets_in_output".to_string()));
}

#[test]
fn test_filter_secrets_detects_aws_key() {
    let msgs = vec![ChatMessage::tool_result(
        "tc1",
        "Found key: AKIAIOSFODNN7EXAMPLE in config",
    )];
    let result = mock_agent_result(msgs, None);
    assert_eq!(filter_secrets_in_output(&result), Some("secrets_in_output".to_string()));
}

#[test]
fn test_filter_secrets_passes_clean_output() {
    let msgs = vec![ChatMessage::tool_result("tc1", "File contents: hello world")];
    let result = mock_agent_result(msgs, None);
    assert_eq!(filter_secrets_in_output(&result), None);
}

// ---------------------------------------------------------------------------
// run_filters
// ---------------------------------------------------------------------------

#[test]
fn test_run_filters_collects_all_reasons() {
    // AgentResult with no tool calls and empty final_response triggers no_reasoning.
    // Also inject all-error tool results to also trigger error_only.
    // But error_only requires Tool role messages, while no_reasoning checks tool_calls field.
    // Craft a result that triggers BOTH: no tool_calls on assistant msg + all Tool results error.
    let msgs = vec![
        ChatMessage::tool_result("tc1", "Error: failed"),
    ];
    let result = mock_agent_result(msgs, None);
    let registry = registry_with("web_read");
    let quality = run_filters(&result, &registry);
    assert!(!quality.passed);
    // Should contain no_reasoning_steps (no tool_calls, empty final_response)
    // AND error_only_trajectory (all tool results are errors)
    assert!(quality.reasons.contains(&"no_reasoning_steps".to_string()), "expected no_reasoning_steps in {:?}", quality.reasons);
    assert!(quality.reasons.contains(&"error_only_trajectory".to_string()), "expected error_only_trajectory in {:?}", quality.reasons);
}

#[test]
fn test_run_filters_passes_clean_result() {
    let registry = registry_with("web_read");
    let msgs = vec![
        ChatMessage::assistant_tool_calls(vec![make_tool_call("web_read")]),
        ChatMessage::tool_result("tc1", "Page content: hello world"),
    ];
    let result = mock_agent_result(msgs, Some("I found the information you need.".to_string()));
    let quality = run_filters(&result, &registry);
    assert!(quality.passed, "expected passed=true, got reasons: {:?}", quality.reasons);
    assert!(quality.reasons.is_empty());
}

// ---------------------------------------------------------------------------
// runner tests (Task 2)
// ---------------------------------------------------------------------------

use super::runner::reject_file_path;
use std::path::PathBuf;

#[test]
fn test_reject_file_path_derivation() {
    let output = PathBuf::from("results/output.jsonl");
    let reject = reject_file_path(&output);
    assert_eq!(reject, PathBuf::from("results/output_rejected.jsonl"));
}

#[test]
fn test_batch_run_record_serialization() {
    let record = BatchRunRecord {
        id: "abc123".to_string(),
        input_file: "test.jsonl".to_string(),
        output_file: "output.jsonl".to_string(),
        total_entries: 10,
        completed: 10,
        passed: 8,
        rejected: 2,
        started_at: "2026-04-10T00:00:00Z".to_string(),
        finished_at: Some("2026-04-10T00:05:00Z".to_string()),
        status: "completed".to_string(),
    };
    let json = serde_json::to_string(&record).unwrap();
    assert!(json.contains("\"passed\":8"));
    assert!(json.contains("\"rejected\":2"));
    let parsed: BatchRunRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.passed, 8);
}

// ---------------------------------------------------------------------------
// UAT gap regression tests (Phase 10 Plan 03)
// ---------------------------------------------------------------------------

#[test]
fn test_filter_secrets_detects_in_assistant_text() {
    // UAT gap: secrets filter must scan Role::Assistant messages, not just Role::Tool
    let mut msg = ChatMessage::assistant("Found key: AKIAIOSFODNN7EXAMPLE in the config file");
    msg.role = ironhermes_core::Role::Assistant;
    let result = mock_agent_result(vec![msg], None);
    assert_eq!(
        filter_secrets_in_output(&result),
        Some("secrets_in_output".to_string()),
        "secrets filter should detect AWS key in assistant message"
    );
}

#[test]
fn test_run_filters_rejects_text_only_no_tools() {
    // UAT gap: run_filters must reject text-only responses with no tool calls
    let registry = registry_with("web_read");
    let result = mock_agent_result(
        vec![ChatMessage::user("hello")],
        Some("I can help you with many things! Just ask me anything.".to_string()),
    );
    let quality = run_filters(&result, &registry);
    assert!(!quality.passed, "text-only response with no tool calls should be rejected");
    assert!(
        quality.reasons.contains(&"no_reasoning_steps".to_string()),
        "expected no_reasoning_steps in {:?}",
        quality.reasons
    );
}

// ---------------------------------------------------------------------------
// Task 1 (Plan 04): cancel sentinel timestamp-guard regression tests
// ---------------------------------------------------------------------------

use super::runner::clean_stale_sentinel;

#[test]
fn test_stale_sentinel_removed_at_startup() {
    // Create a cancel file with a backdate mtime (2 seconds in the past)
    let dir = tempfile::tempdir().unwrap();
    let sentinel = dir.path().join("cancel");
    std::fs::write(&sentinel, "").unwrap();

    // Backdate the file mtime by 2 seconds
    let two_secs_ago = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(2))
        .unwrap();
    let file = std::fs::File::options().write(true).open(&sentinel).unwrap();
    file.set_modified(two_secs_ago).unwrap();
    drop(file);

    // run_start is now (after the stale file)
    let run_start = std::time::SystemTime::now();
    clean_stale_sentinel(&sentinel, run_start);

    assert!(!sentinel.exists(), "stale sentinel should have been removed");
}

#[test]
fn test_fresh_sentinel_preserved_at_startup() {
    // Create a cancel file with current mtime (fresh — created concurrently)
    let dir = tempfile::tempdir().unwrap();
    let sentinel = dir.path().join("cancel");
    std::fs::write(&sentinel, "").unwrap();

    // run_start is 2 seconds in the past — so the file appears newer
    let run_start = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(2))
        .unwrap();
    clean_stale_sentinel(&sentinel, run_start);

    assert!(sentinel.exists(), "fresh sentinel should NOT be removed");
}

// ---------------------------------------------------------------------------
// Task 2 (Plan 04): filter_no_reasoning content-length fallback regression tests
// ---------------------------------------------------------------------------

#[test]
fn test_filter_no_reasoning_passes_substantive_text() {
    // A real text-only answer with >=100 chars in an assistant message should pass
    let sky_answer = "The sky appears blue because of a phenomenon called Rayleigh scattering. \
        When sunlight enters the atmosphere, it collides with gas molecules. \
        Blue light has a shorter wavelength and scatters more than red light, \
        which is why we see a blue sky during the day.";
    assert!(sky_answer.len() >= 100, "test string must be >=100 chars");
    let msg = ChatMessage::assistant(sky_answer);
    let result = mock_agent_result(vec![msg], None);
    assert_eq!(
        filter_no_reasoning(&result),
        None,
        "substantive text-only answer should pass the no_reasoning filter"
    );
}

#[test]
fn test_filter_no_reasoning_passes_long_final_response() {
    // No tool calls, but final_response is 150+ chars — should pass
    let long_response = "The process of photosynthesis is how plants convert sunlight into food. \
        Using chlorophyll in their leaves, plants absorb carbon dioxide from the air \
        and water from the soil, then use solar energy to produce glucose and oxygen.";
    assert!(long_response.len() >= 100, "test string must be >=100 chars");
    let result = mock_agent_result(vec![], Some(long_response.to_string()));
    assert_eq!(
        filter_no_reasoning(&result),
        None,
        "long final_response without tool calls should pass"
    );
}

#[test]
fn test_filter_no_reasoning_rejects_short_text_no_tools() {
    // Short assistant text, no tool calls — must be rejected
    let msg = ChatMessage::assistant("Sure, I can help with that.");
    let result = mock_agent_result(vec![msg], None);
    assert_eq!(
        filter_no_reasoning(&result),
        Some("no_reasoning_steps".to_string()),
        "short text-only response without tool calls should be rejected"
    );
}

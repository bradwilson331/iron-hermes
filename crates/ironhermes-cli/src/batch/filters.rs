use ironhermes_agent::AgentResult;
use ironhermes_core::Role;
use ironhermes_tools::ToolRegistry;
use regex::RegexSet;
use std::sync::LazyLock;

use super::types::QualityResult;

/// Credential/secret patterns for detecting leaked secrets in tool output (D-12 criterion 4).
/// These target actual secret formats, NOT the prompt injection patterns in context_scanner.rs.
static SECRET_PATTERNS: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSet::new([
        // Stripe-style API keys: sk-live-..., pk-test-..., etc.
        "(?i)(sk|pk)[-_](?:live|test|prod)[a-zA-Z0-9_\\-]{20,}",
        // JWT Bearer tokens
        "(?i)Bearer\\s+ey[a-zA-Z0-9_\\-\\.]{20,}",
        // Generic API key assignments
        "(?i)(api[_-]?key|api[_-]?secret|access[_-]?token)\\s*[:=]\\s*['\"]?[a-zA-Z0-9_\\-]{20,}",
        // GitHub personal access tokens
        "(?i)ghp_[a-zA-Z0-9]{36}",
        // Slack tokens
        "(?i)xox[bpsa]-[a-zA-Z0-9\\-]{10,}",
        // AWS access key IDs
        "(?i)AKIA[A-Z0-9]{16}",
        // PEM private keys
        "-----BEGIN (RSA )?PRIVATE KEY-----",
    ])
    .expect("SECRET_PATTERNS regex compilation failed")
});

/// D-12 criterion 1: Detect tool calls to tools not in the registry.
pub fn filter_hallucinated_tools(result: &AgentResult, registry: &ToolRegistry) -> Option<String> {
    let known: Vec<&str> = registry.list_tools();
    for msg in &result.messages {
        if let Some(tool_calls) = &msg.tool_calls {
            for tc in tool_calls {
                if !known.contains(&tc.function.name.as_str()) {
                    return Some(format!("hallucinated_tool:{}", tc.function.name));
                }
            }
        }
    }
    None
}

/// D-12 criterion 2: Reject if result has zero tool calls AND no substantive text.
pub fn filter_no_reasoning(result: &AgentResult) -> Option<String> {
    let has_tool_calls = result.messages.iter().any(|m| {
        m.tool_calls
            .as_ref()
            .is_some_and(|tc| !tc.is_empty())
    });
    let has_text = result
        .final_response
        .as_ref()
        .is_some_and(|r| r.trim().len() > 10);
    if !has_tool_calls && !has_text {
        return Some("no_reasoning_steps".to_string());
    }
    None
}

/// D-12 criterion 3: Reject if every tool call produced an error result.
pub fn filter_error_only(result: &AgentResult) -> Option<String> {
    let tool_results: Vec<&str> = result
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .filter_map(|m| m.content_text())
        .collect();

    if tool_results.is_empty() {
        return None; // No tool calls = not an error-only trajectory
    }

    let all_errors = tool_results.iter().all(|content| {
        content.starts_with("Error:")
            || content.starts_with("error:")
            || content.contains("failed")
            || content.contains("BLOCKED")
    });

    if all_errors {
        return Some("error_only_trajectory".to_string());
    }
    None
}

/// D-12 criterion 4: Detect secrets/credentials leaked in tool output.
/// Uses SECRET_PATTERNS regex set -- distinct from context_scanner.rs THREAT_PATTERNS.
pub fn filter_secrets_in_output(result: &AgentResult) -> Option<String> {
    for msg in &result.messages {
        if msg.role == Role::Tool {
            if let Some(text) = msg.content_text() {
                if SECRET_PATTERNS.is_match(text) {
                    return Some("secrets_in_output".to_string());
                }
            }
        }
    }
    None
}

/// Run all quality filters on an AgentResult. Returns QualityResult with all matching reasons.
/// Does NOT short-circuit -- collects all applicable rejection reasons (D-13).
pub fn run_filters(result: &AgentResult, registry: &ToolRegistry) -> QualityResult {
    let mut reasons = Vec::new();

    if let Some(r) = filter_hallucinated_tools(result, registry) {
        reasons.push(r);
    }
    if let Some(r) = filter_no_reasoning(result) {
        reasons.push(r);
    }
    if let Some(r) = filter_error_only(result) {
        reasons.push(r);
    }
    if let Some(r) = filter_secrets_in_output(result) {
        reasons.push(r);
    }

    QualityResult {
        passed: reasons.is_empty(),
        reasons,
    }
}

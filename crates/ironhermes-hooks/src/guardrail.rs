use crate::config::{ErrorDetailLevel, HooksConfig};

/// The outcome of a guardrail check on a tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum GuardrailDecision {
    /// Allow the tool call to proceed.
    Allow,
    /// Log a warning and emit a hook event, but allow the call to proceed.
    Warn { reason: String },
    /// Block the tool call. The reason is returned to the agent as an error.
    Block { reason: String },
}

/// A guardrail hook that can intercept tool calls before dispatch.
///
/// Implementations inspect the tool name and arguments and return a decision.
/// This method is synchronous — no I/O should be performed here.
pub trait GuardrailHook: Send + Sync {
    /// Inspect a tool call and decide whether to allow, warn, or block.
    fn check(&self, tool_name: &str, args: &serde_json::Value) -> GuardrailDecision;

    /// Human-readable name for this guardrail (used in log messages).
    fn name(&self) -> &str;
}

/// Config-driven blocklist guardrail: blocks tools by name.
///
/// Per D-05: registered first in the guardrail chain so blocked tools are
/// rejected before any custom trait-based guardrails run.
pub struct BlocklistGuardrail {
    blocked_tools: Vec<String>,
}

impl BlocklistGuardrail {
    /// Create a blocklist guardrail from an explicit list of tool names.
    pub fn new(blocked_tools: Vec<String>) -> Self {
        Self { blocked_tools }
    }

    /// Create a blocklist guardrail from the `blocked_tools` field in `HooksConfig`.
    pub fn from_config(config: &HooksConfig) -> Self {
        Self::new(config.blocked_tools.clone())
    }
}

impl GuardrailHook for BlocklistGuardrail {
    fn check(&self, tool_name: &str, _args: &serde_json::Value) -> GuardrailDecision {
        if self.blocked_tools.iter().any(|b| b == tool_name) {
            GuardrailDecision::Block {
                reason: format!("tool '{}' is on the blocklist", tool_name),
            }
        } else {
            GuardrailDecision::Allow
        }
    }

    fn name(&self) -> &str {
        "blocklist"
    }
}

/// Format the error message returned to the agent when a tool call is blocked.
///
/// Per D-06: `ErrorDetailLevel::Full` includes the tool name and reason for
/// developer convenience; `ErrorDetailLevel::Minimal` returns a generic message
/// for high-security deployments where leaking tool names is undesirable (T-06-05).
pub fn format_guardrail_error(
    tool_name: &str,
    reason: &str,
    guardrail_name: &str,
    detail_level: &ErrorDetailLevel,
) -> String {
    match detail_level {
        ErrorDetailLevel::Full => {
            format!(
                "Tool '{}' blocked by guardrail '{}': {}",
                tool_name, guardrail_name, reason
            )
        }
        ErrorDetailLevel::Minimal => "Tool call blocked by security policy".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ErrorDetailLevel;

    fn json_null() -> serde_json::Value {
        serde_json::Value::Null
    }

    #[test]
    fn test_blocklist_blocks_named_tool() {
        let guardrail = BlocklistGuardrail::new(vec!["terminal".to_string()]);
        let decision = guardrail.check("terminal", &json_null());
        assert!(
            matches!(decision, GuardrailDecision::Block { .. }),
            "expected Block, got {decision:?}"
        );
    }

    #[test]
    fn test_blocklist_allows_unlisted_tool() {
        let guardrail = BlocklistGuardrail::new(vec!["terminal".to_string()]);
        let decision = guardrail.check("read_file", &json_null());
        assert_eq!(decision, GuardrailDecision::Allow);
    }

    #[test]
    fn test_blocklist_empty_allows_all() {
        let guardrail = BlocklistGuardrail::new(vec![]);
        assert_eq!(guardrail.check("terminal", &json_null()), GuardrailDecision::Allow);
        assert_eq!(guardrail.check("write_file", &json_null()), GuardrailDecision::Allow);
        assert_eq!(guardrail.check("any_tool", &json_null()), GuardrailDecision::Allow);
    }

    #[test]
    fn test_format_guardrail_error_full() {
        let msg = format_guardrail_error("terminal", "on the blocklist", "blocklist", &ErrorDetailLevel::Full);
        assert!(msg.contains("terminal"), "expected tool name in full message: {msg}");
        assert!(msg.contains("on the blocklist"), "expected reason in full message: {msg}");
        assert!(msg.contains("blocklist"), "expected guardrail name in full message: {msg}");
    }

    #[test]
    fn test_format_guardrail_error_minimal() {
        let msg = format_guardrail_error("terminal", "on the blocklist", "blocklist", &ErrorDetailLevel::Minimal);
        assert_eq!(msg, "Tool call blocked by security policy");
        assert!(!msg.contains("terminal"), "tool name must not appear in minimal message: {msg}");
    }

    #[test]
    fn test_guardrail_decision_warn() {
        let warn = GuardrailDecision::Warn {
            reason: "suspicious args".to_string(),
        };
        assert!(!matches!(warn, GuardrailDecision::Block { .. }));
        assert!(!matches!(warn, GuardrailDecision::Allow));
        assert!(matches!(warn, GuardrailDecision::Warn { .. }));
    }

    #[test]
    fn test_blocklist_from_config() {
        let mut config = HooksConfig::default();
        config.blocked_tools = vec!["terminal".to_string(), "write_file".to_string()];
        let guardrail = BlocklistGuardrail::from_config(&config);
        assert!(matches!(guardrail.check("terminal", &json_null()), GuardrailDecision::Block { .. }));
        assert!(matches!(guardrail.check("write_file", &json_null()), GuardrailDecision::Block { .. }));
        assert_eq!(guardrail.check("read_file", &json_null()), GuardrailDecision::Allow);
    }
}

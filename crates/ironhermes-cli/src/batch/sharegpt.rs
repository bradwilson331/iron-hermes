use ironhermes_core::ChatMessage;
use super::types::ShareGptTurn;

/// Convert AgentResult messages to ShareGPT conversation turns (D-07, D-08).
/// Roles: User -> "human", Assistant text -> "gpt", tool_calls -> "tool_call",
/// Tool -> "tool_response". System messages are skipped.
pub fn messages_to_sharegpt(_messages: &[ChatMessage]) -> Vec<ShareGptTurn> {
    todo!("Implemented in Task 2")
}

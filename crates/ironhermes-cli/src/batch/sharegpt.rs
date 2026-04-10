use ironhermes_core::{ChatMessage, Role};
use super::types::ShareGptTurn;

/// Convert AgentResult messages to ShareGPT conversation turns (D-07, D-08).
/// Roles: User -> "human", Assistant text -> "gpt", tool_calls -> "tool_call",
/// Tool -> "tool_response". System messages are skipped.
pub fn messages_to_sharegpt(messages: &[ChatMessage]) -> Vec<ShareGptTurn> {
    let mut turns = Vec::new();
    for msg in messages {
        match msg.role {
            Role::User => {
                if let Some(text) = msg.content_text() {
                    turns.push(ShareGptTurn {
                        from: "human".to_string(),
                        value: text.to_string(),
                    });
                }
            }
            Role::Assistant => {
                // Text content as "gpt" turn
                if let Some(text) = msg.content_text() {
                    if !text.is_empty() {
                        turns.push(ShareGptTurn {
                            from: "gpt".to_string(),
                            value: text.to_string(),
                        });
                    }
                }
                // Tool calls as separate "tool_call" turns (D-07)
                if let Some(tool_calls) = &msg.tool_calls {
                    for tc in tool_calls {
                        turns.push(ShareGptTurn {
                            from: "tool_call".to_string(),
                            value: serde_json::to_string(tc).unwrap_or_default(),
                        });
                    }
                }
            }
            Role::Tool => {
                turns.push(ShareGptTurn {
                    from: "tool_response".to_string(),
                    value: msg.content_text().unwrap_or("").to_string(),
                });
            }
            Role::System => {} // System prompt excluded from ShareGPT trajectory
        }
    }
    turns
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// Chat Messages (OpenAI-compatible format)
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl MessageContent {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MessageContent::Text(s) => Some(s),
            MessageContent::Parts(parts) => parts.iter().find_map(|p| {
                if let ContentPart::Text { text } = p {
                    Some(text.as_str())
                } else {
                    None
                }
            }),
        }
    }

    pub fn text(s: impl Into<String>) -> Self {
        MessageContent::Text(s.into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

// =============================================================================
// Tool Calls
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

// =============================================================================
// Tool Schema (OpenAI-compatible function definitions)
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    pub function: FunctionSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolSchema {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            schema_type: "function".to_string(),
            function: FunctionSchema {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

// =============================================================================
// Chat Completion Request/Response (OpenAI-compatible)
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolSchema>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: usize,
    pub message: ChatMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<usize>,
}

// =============================================================================
// Streaming Types
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatStreamChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<StreamChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChoice {
    pub index: usize,
    pub delta: StreamDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<StreamToolCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamToolCall {
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub call_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<StreamFunctionCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamFunctionCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

// =============================================================================
// Platform / Gateway Types
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Local,
    Telegram,
    Discord,
    Whatsapp,
    Slack,
    Signal,
    Matrix,
    Mattermost,
    Email,
    Sms,
    Dingtalk,
    Feishu,
    Wecom,
    HomeAssistant,
    Webhook,
    ApiServer,
    Web, // Phase 25.5: Dioxus web UI sessions
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Platform::Local => "local",
            Platform::Telegram => "telegram",
            Platform::Discord => "discord",
            Platform::Whatsapp => "whatsapp",
            Platform::Slack => "slack",
            Platform::Signal => "signal",
            Platform::Matrix => "matrix",
            Platform::Mattermost => "mattermost",
            Platform::Email => "email",
            Platform::Sms => "sms",
            Platform::Dingtalk => "dingtalk",
            Platform::Feishu => "feishu",
            Platform::Wecom => "wecom",
            Platform::HomeAssistant => "homeassistant",
            Platform::Webhook => "webhook",
            Platform::ApiServer => "api_server",
            Platform::Web => "web",
        };
        write!(f, "{}", s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEvent {
    pub platform: Platform,
    pub message_id: String,
    pub chat_id: String,
    pub sender_id: String,
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub thread_id: Option<String>,
    #[serde(default = "default_chat_type")]
    pub chat_type: String,
    pub chat_name: Option<String>,
    pub sender_name: Option<String>,
    pub replied_to_id: Option<String>,
}

fn default_chat_type() -> String {
    "dm".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub url: Option<String>,
    pub data: Option<Vec<u8>>,
    pub mime_type: Option<String>,
    pub filename: Option<String>,
    /// Platform-specific file identifier (e.g., Telegram file_id for deferred download).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageResponse {
    pub message_id: String,
    pub chat_id: String,
    pub platform: Platform,
}

// =============================================================================
// Helper constructors
// =============================================================================

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
        }
    }

    pub fn content_text(&self) -> Option<&str> {
        self.content.as_ref().and_then(|c| c.as_text())
    }
}

// =============================================================================
// Phase 25.1 GAP-7: tool-call pairing invariant
// =============================================================================

/// Phase 25.1 GAP-7: validate the OpenAI strict tool-pair invariant before
/// any LLM request goes on the wire.
///
/// SEMANTICS (strict, NOT flat-set-difference):
///   Walk messages in order. Maintain `pending_ids: HashSet<String>` of
///   tool_call ids the most-recent assistant block emitted. Then:
///   - On a NEW assistant message:
///       * If `pending_ids` is non-empty → Err (previous assistant's tool
///         calls were never answered; OpenAI rejects this even if a
///         matching tool message appears LATER in the array).
///       * Otherwise, replace `pending_ids` with the ids from this
///         message's tool_calls (or empty if no tool_calls).
///   - On a TOOL message:
///       * If `tool_call_id` is missing → Err.
///       * If `tool_call_id` not in `pending_ids` → Err (orphan tool, e.g.
///         the id belonged to a prior assistant whose pending set was
///         already cleared by an interjecting assistant).
///       * Otherwise remove the id from `pending_ids`.
///   - On a USER or SYSTEM message:
///       * If `pending_ids` is non-empty → Err (a non-tool message
///         interjected before all tool results arrived).
///   - At END of the array:
///       * If `pending_ids` is non-empty → Err.
///
/// Returns Err with a human-readable diagnostic naming the offending id and
/// (where applicable) the message-array index. The WARN log at the call
/// site surfaces this diagnostic so debugging is one-line.
///
/// Why strict and not flat-set: a flat `HashSet<String>` across the whole
/// array would falsely accept `[asst[c1], asst[no-calls], tool(c1)]` even
/// though OpenAI's invariant is "answered BEFORE the next assistant", and
/// the provider rejects this sequence. The flat semantics is what the prior
/// `tool_pair::check_orphan_invariant` used; we fix it here and re-route
/// that wrapper to delegate so there is a SINGLE source of truth for
/// pairing logic across both crates.
pub fn validate_tool_call_pairing(messages: &[ChatMessage]) -> Result<(), String> {
    use std::collections::HashSet;
    let mut pending: HashSet<String> = HashSet::new();
    for (idx, msg) in messages.iter().enumerate() {
        match msg.role {
            Role::Assistant => {
                if !pending.is_empty() {
                    let example = pending.iter().next().cloned().unwrap_or_default();
                    return Err(format!(
                        "orphan tool_call_id '{example}' (and {} other(s)) at messages[{idx}]: a new assistant message arrived before tool messages answered the prior assistant block",
                        pending.len().saturating_sub(1)
                    ));
                }
                pending.clear();
                if let Some(ref calls) = msg.tool_calls {
                    for c in calls {
                        pending.insert(c.id.clone());
                    }
                }
            }
            Role::Tool => {
                let id = msg.tool_call_id.as_ref().ok_or_else(|| {
                    format!("tool message at messages[{idx}] has no tool_call_id")
                })?;
                if !pending.remove(id) {
                    return Err(format!(
                        "orphan tool_call_id '{id}' at messages[{idx}]: tool message has no preceding (still-pending) assistant.tool_calls entry"
                    ));
                }
            }
            Role::User | Role::System => {
                if !pending.is_empty() {
                    let example = pending.iter().next().cloned().unwrap_or_default();
                    return Err(format!(
                        "orphan tool_call_id '{example}' (and {} other(s)) at messages[{idx}]: a {:?} message arrived while tool calls were still pending",
                        pending.len().saturating_sub(1),
                        msg.role
                    ));
                }
            }
        }
    }
    if let Some(id) = pending.iter().next() {
        return Err(format!(
            "orphan tool_call_id '{id}': assistant.tool_calls entry has no following tool message before end-of-history"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tool_pair_invariant_tests {
    use super::*;

    fn tc(id: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "fn".to_string(),
                arguments: "{}".to_string(),
            },
        }
    }

    #[test]
    fn validate_tool_call_pairing_passes_clean_history() {
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant("hi"),
            ChatMessage::user("u2"),
            ChatMessage::assistant_tool_calls(vec![tc("c1")]),
            ChatMessage::tool_result("c1", "r"),
            ChatMessage::assistant("done"),
        ];
        assert!(validate_tool_call_pairing(&msgs).is_ok());
    }

    #[test]
    fn validate_tool_call_pairing_fails_missing_tool_result_names_orphan() {
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant_tool_calls(vec![tc("call_3PlsiYtJbAmwCtvtxjcIhI3G")]),
            ChatMessage::user("next"),
        ];
        let err = validate_tool_call_pairing(&msgs).expect_err("must err");
        assert!(
            err.contains("call_3PlsiYtJbAmwCtvtxjcIhI3G"),
            "diagnostic must name the orphan id; got: {err}"
        );
    }

    #[test]
    fn validate_tool_call_pairing_fails_tool_without_assistant() {
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::tool_result("dangling", "r"),
            ChatMessage::assistant("done"),
        ];
        let err = validate_tool_call_pairing(&msgs).expect_err("must err");
        assert!(
            err.contains("dangling"),
            "diagnostic must name the dangling id; got: {err}"
        );
    }

    #[test]
    fn validate_tool_call_pairing_handles_parallel_tool_calls() {
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant_tool_calls(vec![tc("a"), tc("b")]),
            ChatMessage::tool_result("a", "r1"),
            ChatMessage::tool_result("b", "r2"),
            ChatMessage::assistant("done"),
        ];
        assert!(validate_tool_call_pairing(&msgs).is_ok());
    }

    #[test]
    fn validate_tool_call_pairing_partial_parallel_orphans() {
        // b is orphaned — closed by next assistant, NOT end-of-array
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant_tool_calls(vec![tc("a"), tc("b")]),
            ChatMessage::tool_result("a", "r1"),
            ChatMessage::assistant("done"),
        ];
        let err = validate_tool_call_pairing(&msgs).expect_err("must err");
        assert!(
            err.contains("b"),
            "diagnostic must reference 'b'; got: {err}"
        );
    }

    #[test]
    fn validate_tool_call_pairing_strict_rejects_late_tool_after_new_assistant() {
        // STRICT VS FLAT divergence test. Flat-set Ok; strict MUST Err.
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant_tool_calls(vec![tc("c1")]),
            ChatMessage::assistant("interjection"),
            ChatMessage::tool_result("c1", "late"),
        ];
        let err = validate_tool_call_pairing(&msgs).expect_err("must err");
        assert!(
            err.contains("c1"),
            "diagnostic must reference 'c1'; got: {err}"
        );
    }

    #[test]
    fn validate_tool_call_pairing_strict_rejects_user_arriving_with_pending() {
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant_tool_calls(vec![tc("c1")]),
            ChatMessage::user("ignoring the pending tool"),
        ];
        let err = validate_tool_call_pairing(&msgs).expect_err("must err");
        assert!(
            err.contains("c1"),
            "diagnostic must reference 'c1'; got: {err}"
        );
    }

    #[test]
    fn validate_tool_call_pairing_unknown_tool_call_id() {
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u"),
            ChatMessage::assistant_tool_calls(vec![tc("c1")]),
            ChatMessage::tool_result("c2", "r"),
        ];
        let err = validate_tool_call_pairing(&msgs).expect_err("must err");
        assert!(
            err.contains("c2"),
            "diagnostic must reference 'c2'; got: {err}"
        );
    }
}

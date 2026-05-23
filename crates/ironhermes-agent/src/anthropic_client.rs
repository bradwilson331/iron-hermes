use anyhow::{Context, Result};
use futures::StreamExt;
use ironhermes_core::{
    ChatChoice, ChatMessage, ChatResponse, ContentPart, FunctionCall, ImageUrl, MessageContent,
    Role, ToolCall, ToolSchema, Usage,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use tracing::{debug, warn};

use crate::client::StreamEvent;

// =============================================================================
// Anthropic request/response types
// =============================================================================

#[derive(Debug, Clone, Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    max_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String, // "user" or "assistant" only
    content: AnthropicContent,
}

/// Content of an Anthropic message: either a plain string or a list of content blocks.
#[derive(Debug, Clone)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl Serialize for AnthropicContent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            AnthropicContent::Text(s) => serializer.serialize_str(s),
            AnthropicContent::Blocks(blocks) => blocks.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for AnthropicContent {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let val = serde_json::Value::deserialize(deserializer)?;
        match val {
            serde_json::Value::String(s) => Ok(AnthropicContent::Text(s)),
            serde_json::Value::Array(arr) => {
                let blocks: Vec<ContentBlock> =
                    serde_json::from_value(serde_json::Value::Array(arr))
                        .map_err(serde::de::Error::custom)?;
                Ok(AnthropicContent::Blocks(blocks))
            }
            other => Err(serde::de::Error::custom(format!(
                "Expected string or array for AnthropicContent, got: {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        source: ImageSource, // Phase 25.1 OQ-2: multimodal user input
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ImageSource {
    /// Base64-encoded inline image. browser_vision sends this for full-page screenshots.
    Base64 {
        media_type: String, // "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        data: String,       // base64 payload (no "data:..." prefix)
    },
    /// URL-source (Anthropic supports this in newer API versions).
    Url { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

// --- Response types ---

#[derive(Debug, Clone, Deserialize)]
struct AnthropicResponse {
    id: String,
    content: Vec<ResponseContentBlock>,
    model: String,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Clone, Deserialize)]
struct AnthropicUsage {
    input_tokens: usize,
    output_tokens: usize,
}

// =============================================================================
// SSE types for streaming
// =============================================================================

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicSseEvent {
    MessageStart {
        message: serde_json::Value,
    },
    ContentBlockStart {
        index: usize,
        content_block: SseContentBlock,
    },
    ContentBlockDelta {
        index: usize,
        delta: SseDelta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        delta: SseMessageDelta,
        #[serde(default)]
        usage: Option<SseUsage>,
    },
    MessageStop,
    Ping,
    Error {
        error: serde_json::Value,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SseContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SseDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Clone, Deserialize)]
struct SseMessageDelta {
    stop_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SseUsage {
    output_tokens: Option<usize>,
}

// =============================================================================
// Credential discovery (D-09 — startup-only, no OAuth refresh)
// =============================================================================

/// Discover the Anthropic API credential to use.
///
/// Priority order (D-09, T-12-06):
/// 1. `config_api_key` if provided and non-empty
/// 2. `ANTHROPIC_API_KEY` environment variable
/// 3. `~/.claude/credentials.json` `oauth.accessToken` field
///
/// Returns `None` if all sources fail.
/// This is called once at startup. No expiry check, no token refresh (deferred per D-09).
pub fn discover_anthropic_credential(config_api_key: Option<&str>) -> Option<String> {
    // 1. Config api_key
    if let Some(key) = config_api_key {
        if !key.is_empty() {
            return Some(key.to_string());
        }
    }

    // 2. Environment variable
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            return Some(key);
        }
    }

    // 3. ~/.claude/credentials.json oauth.accessToken
    let home = std::env::var("HOME").ok()?;
    let creds_path = std::path::Path::new(&home)
        .join(".claude")
        .join("credentials.json");
    let content = std::fs::read_to_string(&creds_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let token = json
        .get("oauth")
        .and_then(|o| o.get("accessToken"))
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .map(String::from)?;

    Some(token)
}

// =============================================================================
// Adapter functions
// =============================================================================

/// Extract system messages and convert the remaining messages to Anthropic format.
///
/// Returns `(system_prompt, anthropic_messages)`.
///
/// Translation rules:
/// - `system` messages: concatenated into system prompt (not included in messages)
/// - `user` messages: role="user", content as text block
/// - `assistant` messages with tool_calls: content blocks (text first if any, then tool_use blocks)
/// - `tool` messages: role="user" with tool_result content block
/// - Consecutive same-role messages are merged into a single message
pub fn adapt_messages(messages: &[ChatMessage]) -> (Option<String>, Vec<AnthropicMessage>) {
    // Extract system messages
    let system_parts: Vec<String> = messages
        .iter()
        .filter(|m| m.role == Role::System)
        .filter_map(|m| {
            m.content
                .as_ref()
                .and_then(|c| c.as_text())
                .map(String::from)
        })
        .collect();

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };

    // Convert non-system messages
    let mut raw_messages: Vec<AnthropicMessage> = Vec::new();

    for msg in messages {
        match msg.role {
            Role::System => continue,
            Role::User => {
                let blocks: Vec<ContentBlock> = match msg.content.as_ref() {
                    Some(MessageContent::Text(t)) => vec![ContentBlock::Text { text: t.clone() }],
                    Some(MessageContent::Parts(parts)) => parts
                        .iter()
                        .filter_map(|p| match p {
                            ContentPart::Text { text } => {
                                Some(ContentBlock::Text { text: text.clone() })
                            }
                            ContentPart::ImageUrl { image_url } => {
                                convert_image_url_to_block(&image_url.url)
                            }
                        })
                        .collect(),
                    None => vec![ContentBlock::Text {
                        text: String::new(),
                    }],
                };
                raw_messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: AnthropicContent::Blocks(blocks),
                });
            }
            Role::Assistant => {
                let mut blocks: Vec<ContentBlock> = Vec::new();

                // Text content first (if any)
                if let Some(text) = msg.content.as_ref().and_then(|c| c.as_text()) {
                    if !text.is_empty() {
                        blocks.push(ContentBlock::Text {
                            text: text.to_string(),
                        });
                    }
                }

                // Tool use blocks
                if let Some(tool_calls) = &msg.tool_calls {
                    for tc in tool_calls {
                        let input: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                        blocks.push(ContentBlock::ToolUse {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            input,
                        });
                    }
                }

                let content = if blocks.len() == 1 {
                    if let ContentBlock::Text { ref text } = blocks[0] {
                        AnthropicContent::Text(text.clone())
                    } else {
                        AnthropicContent::Blocks(blocks)
                    }
                } else {
                    AnthropicContent::Blocks(blocks)
                };

                raw_messages.push(AnthropicMessage {
                    role: "assistant".to_string(),
                    content,
                });
            }
            Role::Tool => {
                let content_text = msg
                    .content
                    .as_ref()
                    .and_then(|c| c.as_text())
                    .unwrap_or("")
                    .to_string();
                let tool_use_id = msg.tool_call_id.clone().unwrap_or_default();

                raw_messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: AnthropicContent::Blocks(vec![ContentBlock::ToolResult {
                        tool_use_id,
                        content: content_text,
                        is_error: None,
                    }]),
                });
            }
        }
    }

    // Merge consecutive same-role messages
    let merged = merge_consecutive_same_role(raw_messages);

    (system, merged)
}

/// Phase 25.1 OQ-2: convert an OpenAI-style ImageUrl.url into an Anthropic Image ContentBlock.
///
/// Accepts:
///   - `data:image/png;base64,<payload>` → `Image { source: Base64 { media_type, data } }`
///   - `data:image/jpeg;base64,<payload>` → ditto
///   - `data:image/gif;base64,<payload>` → ditto
///   - `data:image/webp;base64,<payload>` → ditto
///   - `https://...` or `http://...` → `Image { source: Url { url } }`
///
/// Anything else (malformed/unrecognized) returns None and emits a tracing::warn so the
/// LLM call still proceeds with the rest of the parts intact (graceful degradation).
fn convert_image_url_to_block(url: &str) -> Option<ContentBlock> {
    if let Some(rest) = url.strip_prefix("data:") {
        // Expect "<media>;base64,<data>"
        let (media_with_b64, data) = rest.split_once(',')?;
        let media_type = media_with_b64.strip_suffix(";base64")?.to_string();
        if !["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&media_type.as_str()) {
            tracing::warn!(media = %media_type, "Phase 25.1: unsupported image media-type; skipping image block");
            return None;
        }
        return Some(ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type,
                data: data.to_string(),
            },
        });
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return Some(ContentBlock::Image {
            source: ImageSource::Url {
                url: url.to_string(),
            },
        });
    }
    tracing::warn!(url_prefix = %&url.chars().take(32).collect::<String>(),
        "Phase 25.1: unrecognized ImageUrl scheme; skipping image block");
    None
}

/// Merge consecutive messages with the same role by combining their content blocks.
fn merge_consecutive_same_role(messages: Vec<AnthropicMessage>) -> Vec<AnthropicMessage> {
    let mut result: Vec<AnthropicMessage> = Vec::new();

    for msg in messages {
        if let Some(last) = result.last_mut()
            && last.role == msg.role
        {
            // Merge content blocks
            let new_blocks = content_to_blocks(msg.content);
            match &mut last.content {
                AnthropicContent::Text(t) => {
                    let mut blocks = vec![ContentBlock::Text { text: t.clone() }];
                    blocks.extend(new_blocks);
                    last.content = AnthropicContent::Blocks(blocks);
                }
                AnthropicContent::Blocks(existing) => {
                    existing.extend(new_blocks);
                }
            }
        } else {
            result.push(msg);
        }
    }

    result
}

fn content_to_blocks(content: AnthropicContent) -> Vec<ContentBlock> {
    match content {
        AnthropicContent::Text(t) => vec![ContentBlock::Text { text: t }],
        AnthropicContent::Blocks(blocks) => blocks,
    }
}

/// Convert OpenAI tool schemas to Anthropic tool format.
pub fn adapt_tools(tools: &[ToolSchema]) -> Vec<AnthropicTool> {
    tools
        .iter()
        .map(|t| AnthropicTool {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            input_schema: t.function.parameters.clone(),
        })
        .collect()
}

/// Convert an Anthropic response to OpenAI-compatible ChatResponse.
pub fn parse_anthropic_response(response: &AnthropicResponse) -> (ChatResponse, Option<Usage>) {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    for block in &response.content {
        match block {
            ResponseContentBlock::Text { text } => {
                text_parts.push(text.clone());
            }
            ResponseContentBlock::ToolUse { id, name, input } => {
                let arguments = serde_json::to_string(input).unwrap_or_default();
                tool_calls.push(ToolCall {
                    id: id.clone(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: name.clone(),
                        arguments,
                    },
                });
            }
        }
    }

    let content_text = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join(""))
    };
    let tool_calls_opt = if tool_calls.is_empty() {
        None
    } else {
        Some(tool_calls)
    };

    let message = ChatMessage {
        role: Role::Assistant,
        content: content_text.map(MessageContent::Text),
        tool_calls: tool_calls_opt,
        tool_call_id: None,
        name: None,
        is_recall_context: false,
    };

    let finish_reason = response
        .stop_reason
        .clone()
        .unwrap_or_else(|| "stop".to_string());

    let chat_response = ChatResponse {
        id: response.id.clone(),
        object: "chat.completion".to_string(),
        created: 0,
        model: response.model.clone(),
        choices: vec![ChatChoice {
            index: 0,
            message,
            finish_reason: Some(finish_reason),
        }],
        usage: None, // filled separately
    };

    let usage = Some(Usage {
        prompt_tokens: response.usage.input_tokens,
        completion_tokens: response.usage.output_tokens,
        total_tokens: response.usage.input_tokens + response.usage.output_tokens,
        cache_read_input_tokens: None,
        cache_creation_input_tokens: None,
    });

    (chat_response, usage)
}

// =============================================================================
// AnthropicClient
// =============================================================================

/// HTTP client for the Anthropic Messages API.
///
/// The Debug impl redacts the api_key to prevent accidental key logging (T-12-04).
#[derive(Clone)]
pub struct AnthropicClient {
    http: Client,
    base_url: String,
    api_key: String,
    default_model: String,
}

impl std::fmt::Debug for AnthropicClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicClient")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("default_model", &self.default_model)
            .finish()
    }
}

impl AnthropicClient {
    /// Construct with base_url, api_key, and default model.
    ///
    /// Creates a reqwest Client with `anthropic-version: 2023-06-01` default header
    /// and 30s connect timeout.
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let api_key_str = api_key.into();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "anthropic-version",
            reqwest::header::HeaderValue::from_static("2023-06-01"),
        );

        let http = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .default_headers(headers)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key_str,
            default_model: model.into(),
        }
    }

    /// Non-streaming chat completion.
    ///
    /// Same signature as LlmClient::chat_completion for AnyClient dispatch.
    pub async fn chat_completion(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSchema]>,
        model: Option<&str>,
        max_tokens: Option<usize>,
        _temperature: Option<f64>, // Anthropic supports temperature but we default
        _extra: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<ChatResponse> {
        let (system, adapted_messages) = adapt_messages(messages);
        let adapted_tools = tools.map(adapt_tools);

        let request = AnthropicRequest {
            model: model.unwrap_or(&self.default_model).to_string(),
            messages: adapted_messages,
            system,
            max_tokens: max_tokens.unwrap_or(4096),
            tools: if adapted_tools.as_ref().map(|t| t.is_empty()).unwrap_or(true) {
                None
            } else {
                adapted_tools
            },
            stream: Some(false),
        };

        let url = format!("{}/v1/messages", self.base_url);
        debug!(url = %url, model = %request.model, "Sending Anthropic chat completion request");

        let response = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send Anthropic chat completion request")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic chat completion failed ({}): {}", status, body);
        }

        let anthropic_response: AnthropicResponse = response
            .json()
            .await
            .context("Failed to parse Anthropic chat completion response")?;

        debug!(
            model = %anthropic_response.model,
            stop_reason = ?anthropic_response.stop_reason,
            "Anthropic chat completion response received"
        );

        let (mut chat_response, usage) = parse_anthropic_response(&anthropic_response);
        chat_response.usage = usage;
        Ok(chat_response)
    }

    /// Streaming chat completion.
    ///
    /// Returns a channel receiver for StreamEvents — same return type as LlmClient::chat_completion_stream.
    pub async fn chat_completion_stream(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSchema]>,
        model: Option<&str>,
        max_tokens: Option<usize>,
        _temperature: Option<f64>,
        _extra: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let (system, adapted_messages) = adapt_messages(messages);
        let adapted_tools = tools.map(adapt_tools);

        let request = AnthropicRequest {
            model: model.unwrap_or(&self.default_model).to_string(),
            messages: adapted_messages,
            system,
            max_tokens: max_tokens.unwrap_or(4096),
            tools: if adapted_tools.as_ref().map(|t| t.is_empty()).unwrap_or(true) {
                None
            } else {
                adapted_tools
            },
            stream: Some(true),
        };

        let url = format!("{}/v1/messages", self.base_url);
        debug!(url = %url, model = %request.model, "Sending Anthropic streaming request");

        let response = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send Anthropic streaming request")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic streaming request failed ({}): {}", status, body);
        }

        let (tx, rx) = mpsc::channel(256);

        tokio::spawn(async move {
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();
            let chunk_timeout = Duration::from_secs(60);

            // Track tool call info (index -> (id, name)) for streaming tool use blocks
            let mut tool_call_index: HashMap<usize, (Option<String>, Option<String>)> =
                HashMap::new();

            loop {
                let chunk_result = match timeout(chunk_timeout, byte_stream.next()).await {
                    Ok(Some(result)) => result,
                    Ok(None) => break,
                    Err(_) => {
                        warn!("Anthropic SSE stream read timed out after 60s");
                        break;
                    }
                };

                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("Anthropic stream error: {}", e);
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process SSE events: each event is separated by blank lines
                // Format:
                //   event: <type>\n
                //   data: <json>\n
                //   \n
                while let Some(event_end) = buffer.find("\n\n") {
                    let event_block = buffer[..event_end].to_string();
                    buffer = buffer[event_end + 2..].to_string();

                    // Parse event type and data from the block
                    let mut event_type: Option<String> = None;
                    let mut event_data: Option<String> = None;

                    for line in event_block.lines() {
                        if let Some(et) = line.strip_prefix("event: ") {
                            event_type = Some(et.trim().to_string());
                        } else if let Some(data) = line.strip_prefix("data: ") {
                            event_data = Some(data.trim().to_string());
                        }
                    }

                    let (Some(etype), Some(data)) = (event_type, event_data) else {
                        continue;
                    };

                    // Parse the event by constructing a tagged JSON for serde
                    let tagged = format!(r#"{{"type":"{etype}",{}}}"#, &data[1..data.len() - 1]);
                    let parsed: AnthropicSseEvent = match serde_json::from_str(&tagged) {
                        Ok(e) => e,
                        Err(e) => {
                            debug!(
                                "Failed to parse Anthropic SSE event '{}': {} — data: {}",
                                etype, e, data
                            );
                            continue;
                        }
                    };

                    match parsed {
                        AnthropicSseEvent::ContentBlockStart {
                            index,
                            content_block,
                        } => {
                            match content_block {
                                SseContentBlock::Text { .. } => {} // no-op
                                SseContentBlock::ToolUse { id, name, .. } => {
                                    tool_call_index
                                        .insert(index, (Some(id.clone()), Some(name.clone())));
                                    let _ = tx
                                        .send(StreamEvent::ToolCallDelta {
                                            index,
                                            id: Some(id),
                                            name: Some(name),
                                            arguments: None,
                                        })
                                        .await;
                                }
                            }
                        }
                        AnthropicSseEvent::ContentBlockDelta { index, delta } => match delta {
                            SseDelta::TextDelta { text } => {
                                let _ = tx.send(StreamEvent::ContentDelta(text)).await;
                            }
                            SseDelta::InputJsonDelta { partial_json } => {
                                let _ = tx
                                    .send(StreamEvent::ToolCallDelta {
                                        index,
                                        id: None,
                                        name: None,
                                        arguments: Some(partial_json),
                                    })
                                    .await;
                            }
                        },
                        AnthropicSseEvent::MessageDelta { delta, usage } => {
                            if let Some(u) = usage {
                                let output_tokens = u.output_tokens.unwrap_or(0);
                                let _ = tx
                                    .send(StreamEvent::Usage(Usage {
                                        prompt_tokens: 0,
                                        completion_tokens: output_tokens,
                                        total_tokens: output_tokens,
                                        cache_read_input_tokens: None,
                                        cache_creation_input_tokens: None,
                                    }))
                                    .await;
                            }
                            if let Some(reason) = delta.stop_reason {
                                let _ = tx.send(StreamEvent::Done(Some(reason))).await;
                                return;
                            }
                        }
                        AnthropicSseEvent::MessageStop => {
                            let _ = tx.send(StreamEvent::Done(None)).await;
                            return;
                        }
                        _ => {} // MessageStart, ContentBlockStop, Ping, Error — no-op or ignore
                    }
                }
            }
        });

        Ok(rx)
    }

    pub fn model(&self) -> &str {
        &self.default_model
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::{FunctionSchema, ToolSchema};

    fn make_tool_schema(name: &str, description: &str) -> ToolSchema {
        ToolSchema {
            schema_type: "function".to_string(),
            function: FunctionSchema {
                name: name.to_string(),
                description: description.to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "input": {"type": "string"}
                    }
                }),
            },
        }
    }

    // Test: adapt_messages extracts system messages into separate system string
    #[test]
    fn test_adapt_messages_extracts_system() {
        let messages = vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::user("Hello"),
        ];
        let (system, msgs) = adapt_messages(&messages);
        assert_eq!(system.as_deref(), Some("You are a helpful assistant."));
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
    }

    // Test: adapt_messages converts role:"user" and role:"assistant"
    #[test]
    fn test_adapt_messages_user_and_assistant() {
        let messages = vec![
            ChatMessage::user("Hi"),
            ChatMessage::assistant("Hello there"),
        ];
        let (system, msgs) = adapt_messages(&messages);
        assert!(system.is_none());
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");

        // User should have text content
        match &msgs[0].content {
            AnthropicContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                matches!(&blocks[0], ContentBlock::Text { text } if text == "Hi");
            }
            _ => panic!("Expected blocks for user message"),
        }
    }

    // Test: adapt_messages converts tool_calls to Anthropic tool_use content blocks
    #[test]
    fn test_adapt_messages_tool_calls_to_tool_use() {
        let tool_calls = vec![ToolCall {
            id: "call_123".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "get_weather".to_string(),
                arguments: r#"{"city":"London"}"#.to_string(),
            },
        }];
        let messages = vec![ChatMessage {
            role: Role::Assistant,
            content: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        }];

        let (_, msgs) = adapt_messages(&messages);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "assistant");

        match &msgs[0].content {
            AnthropicContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    ContentBlock::ToolUse { id, name, input } => {
                        assert_eq!(id, "call_123");
                        assert_eq!(name, "get_weather");
                        assert_eq!(input, &serde_json::json!({"city": "London"}));
                    }
                    _ => panic!("Expected ToolUse block"),
                }
            }
            _ => panic!("Expected blocks"),
        }
    }

    // Test: adapt_messages converts role:"tool" messages to role:"user" with tool_result content blocks
    #[test]
    fn test_adapt_messages_tool_result() {
        let messages = vec![ChatMessage::tool_result("call_123", "Sunny, 22C")];
        let (_, msgs) = adapt_messages(&messages);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");

        match &msgs[0].content {
            AnthropicContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => {
                        assert_eq!(tool_use_id, "call_123");
                        assert_eq!(content, "Sunny, 22C");
                    }
                    _ => panic!("Expected ToolResult block"),
                }
            }
            _ => panic!("Expected blocks"),
        }
    }

    // Test: adapt_messages merges consecutive same-role messages
    #[test]
    fn test_adapt_messages_merges_consecutive_same_role() {
        let messages = vec![
            ChatMessage::tool_result("call_1", "Result 1"),
            ChatMessage::tool_result("call_2", "Result 2"),
        ];
        let (_, msgs) = adapt_messages(&messages);
        // Two tool results should be merged into one "user" message with two blocks
        assert_eq!(msgs.len(), 1, "Should merge consecutive same-role messages");
        assert_eq!(msgs[0].role, "user");

        match &msgs[0].content {
            AnthropicContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 2, "Should have 2 tool_result blocks");
            }
            _ => panic!("Expected blocks"),
        }
    }

    // Test: adapt_tools converts Vec<ToolSchema> to Anthropic tool format
    #[test]
    fn test_adapt_tools_conversion() {
        let tools = vec![
            make_tool_schema("search", "Search the web"),
            make_tool_schema("calculator", "Do math"),
        ];
        let adapted = adapt_tools(&tools);
        assert_eq!(adapted.len(), 2);
        assert_eq!(adapted[0].name, "search");
        assert_eq!(adapted[0].description, "Search the web");
        assert_eq!(adapted[1].name, "calculator");
        assert_eq!(adapted[1].description, "Do math");
    }

    // Test: parse_anthropic_response converts Anthropic response to ChatResponse
    #[test]
    fn test_parse_anthropic_response_text() {
        let response = AnthropicResponse {
            id: "msg_01".to_string(),
            content: vec![ResponseContentBlock::Text {
                text: "Hello world".to_string(),
            }],
            model: "claude-3-5-sonnet".to_string(),
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        };

        let (chat_resp, usage) = parse_anthropic_response(&response);
        assert_eq!(chat_resp.id, "msg_01");
        assert_eq!(chat_resp.choices.len(), 1);
        let msg = &chat_resp.choices[0].message;
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content_text(), Some("Hello world"));
        assert!(msg.tool_calls.is_none());

        let u = usage.unwrap();
        assert_eq!(u.prompt_tokens, 10);
        assert_eq!(u.completion_tokens, 5);
        assert_eq!(u.total_tokens, 15);
    }

    // Test: parse_anthropic_response maps tool_calls back to OpenAI format
    #[test]
    fn test_parse_anthropic_response_tool_use() {
        let response = AnthropicResponse {
            id: "msg_02".to_string(),
            content: vec![ResponseContentBlock::ToolUse {
                id: "tool_abc".to_string(),
                name: "get_weather".to_string(),
                input: serde_json::json!({"city": "Paris"}),
            }],
            model: "claude-3-5-sonnet".to_string(),
            stop_reason: Some("tool_use".to_string()),
            usage: AnthropicUsage {
                input_tokens: 20,
                output_tokens: 8,
            },
        };

        let (chat_resp, _) = parse_anthropic_response(&response);
        let msg = &chat_resp.choices[0].message;
        assert!(msg.tool_calls.is_some());
        let tool_calls = msg.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "tool_abc");
        assert_eq!(tool_calls[0].function.name, "get_weather");
        let args: serde_json::Value =
            serde_json::from_str(&tool_calls[0].function.arguments).unwrap();
        assert_eq!(args["city"], "Paris");
    }

    // Test: AnthropicClient::new constructs correctly
    #[test]
    fn test_anthropic_client_new() {
        let client =
            AnthropicClient::new("https://api.anthropic.com", "test-key", "claude-3-5-sonnet");
        assert_eq!(client.base_url(), "https://api.anthropic.com");
        assert_eq!(client.model(), "claude-3-5-sonnet");
        // Debug should redact api_key (T-12-04)
        let debug_str = format!("{:?}", client);
        assert!(
            !debug_str.contains("test-key"),
            "Debug should redact api_key"
        );
        assert!(debug_str.contains("REDACTED"));
    }

    // Test: credential discovery checks config api_key first
    #[test]
    fn test_discover_anthropic_credential_config_key() {
        let result = discover_anthropic_credential(Some("sk-config-key"));
        assert_eq!(result.as_deref(), Some("sk-config-key"));
    }

    // Test: credential discovery falls through to env var when config key is empty
    #[test]
    fn test_discover_anthropic_credential_env_var() {
        // If ANTHROPIC_API_KEY is already set (e.g. real credentials in CI/dev env),
        // verify we return it (discovery works from env). We don't override it since
        // parallel tests could be affected.
        if let Ok(existing) = std::env::var("ANTHROPIC_API_KEY") {
            // Env var is already set — verify discovery returns it
            let result = discover_anthropic_credential(None);
            assert_eq!(result.as_deref(), Some(existing.as_str()));
        } else {
            // No env var set — set a test one, verify, clean up
            // SAFETY: test environment manipulation — checked no pre-existing value
            unsafe {
                std::env::set_var("ANTHROPIC_API_KEY", "sk-env-key-test");
            }
            let result = discover_anthropic_credential(None);
            unsafe {
                std::env::remove_var("ANTHROPIC_API_KEY");
            }
            assert_eq!(result.as_deref(), Some("sk-env-key-test"));
        }
    }

    // Test: empty config key falls through to env var
    #[test]
    fn test_discover_anthropic_credential_empty_config_falls_through() {
        // Empty config key should fall through to env var.
        // If ANTHROPIC_API_KEY is already set, verify we get it back.
        if let Ok(existing) = std::env::var("ANTHROPIC_API_KEY") {
            let result = discover_anthropic_credential(Some(""));
            assert_eq!(result.as_deref(), Some(existing.as_str()));
        } else {
            unsafe {
                std::env::set_var("ANTHROPIC_API_KEY", "sk-env-fallback");
            }
            let result = discover_anthropic_credential(Some(""));
            unsafe {
                std::env::remove_var("ANTHROPIC_API_KEY");
            }
            assert_eq!(result.as_deref(), Some("sk-env-fallback"));
        }
    }

    // Test: assistant message with text AND tool_calls emits text block first, then tool_use
    #[test]
    fn test_adapt_messages_assistant_with_text_and_tool_calls() {
        let tool_calls = vec![ToolCall {
            id: "call_x".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "do_thing".to_string(),
                arguments: "{}".to_string(),
            },
        }];
        let messages = vec![ChatMessage {
            role: Role::Assistant,
            content: Some(MessageContent::Text("Let me help".to_string())),
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        }];

        let (_, msgs) = adapt_messages(&messages);
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            AnthropicContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 2);
                assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "Let me help"));
                assert!(matches!(&blocks[1], ContentBlock::ToolUse { .. }));
            }
            _ => panic!("Expected blocks"),
        }
    }

    #[test]
    fn adapt_messages_user_with_image_data_url_produces_image_block() {
        use ironhermes_core::types::*;
        let user = ChatMessage {
            role: Role::User,
            content: Some(MessageContent::Parts(vec![
                ContentPart::Text {
                    text: "describe".to_string(),
                },
                ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: "data:image/png;base64,iVBORw0KGgo=".to_string(),
                        detail: None,
                    },
                },
            ])),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        };
        let (_sys, msgs) = adapt_messages(&[user]);
        assert_eq!(msgs.len(), 1);
        let json = serde_json::to_string(&msgs[0]).unwrap();
        assert!(
            json.contains(r#""type":"image""#),
            "expected image block, got: {json}"
        );
        assert!(
            json.contains(r#""media_type":"image/png""#),
            "expected media_type, got: {json}"
        );
        assert!(
            json.contains(r#""data":"iVBORw0KGgo=""#),
            "expected base64 data, got: {json}"
        );
        assert!(
            json.contains(r#""text":"describe""#),
            "expected text block alongside image, got: {json}"
        );
    }

    #[test]
    fn adapt_messages_user_text_only_unchanged() {
        use ironhermes_core::types::*;
        let user = ChatMessage::user("hello");
        let (_sys, msgs) = adapt_messages(&[user]);
        let json = serde_json::to_string(&msgs[0]).unwrap();
        assert!(
            json.contains(r#""text":"hello""#),
            "regression: text-only user message must round-trip"
        );
        assert!(
            !json.contains("image"),
            "regression: text-only must not synthesize image block"
        );
    }

    #[test]
    fn adapt_messages_user_with_malformed_data_url_skips_image() {
        use ironhermes_core::types::*;
        let user = ChatMessage {
            role: Role::User,
            content: Some(MessageContent::Parts(vec![
                ContentPart::Text {
                    text: "ok".to_string(),
                },
                ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: "not-a-real-url".to_string(),
                        detail: None,
                    },
                },
            ])),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        };
        let (_sys, msgs) = adapt_messages(&[user]);
        let json = serde_json::to_string(&msgs[0]).unwrap();
        assert!(json.contains(r#""text":"ok""#));
        assert!(
            !json.contains("image"),
            "malformed url MUST skip image block, not crash"
        );
    }
}

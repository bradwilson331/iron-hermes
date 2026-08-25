use anyhow::{Context, Result};
use futures::StreamExt;
use ironhermes_core::config::PromptCachingConfig;
use ironhermes_core::{
    ChatChoice, ChatMessage, ChatResponse, ContentPart, FunctionCall, MessageContent, Role,
    ToolCall, ToolSchema, Usage,
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
pub struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<AnthropicSystem>,
    max_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

/// Phase 36.2 (D-CACHE-01/02): Anthropic system block content.
///
/// Two serialization shapes:
/// - `Text(String)` → plain string (legacy, no caching)
/// - `Blocks(Vec<SystemBlock>)` → array form (required when attaching
///   `cache_control` markers per Anthropic's API)
#[derive(Debug, Clone)]
pub enum AnthropicSystem {
    Text(String),
    Blocks(Vec<SystemBlock>),
}

impl Serialize for AnthropicSystem {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            AnthropicSystem::Text(s) => serializer.serialize_str(s),
            AnthropicSystem::Blocks(blocks) => blocks.serialize(serializer),
        }
    }
}

/// Phase 36.2 (D-CACHE-01): Anthropic system content block. Always `type: "text"`.
///
/// Carries an optional `cache_control` marker — when present, marks this
/// system block as a cache breakpoint.
#[derive(Debug, Clone, Serialize)]
pub struct SystemBlock {
    #[serde(rename = "type")]
    ty: &'static str,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

impl SystemBlock {
    fn text(text: String) -> Self {
        Self {
            ty: "text",
            text,
            cache_control: None,
        }
    }
}

/// Phase 36.2 (D-CACHE-01/02): Anthropic `cache_control` envelope.
///
/// Serializes as `{"type":"ephemeral","ttl":"5m"|"1h"}`. Attached to content
/// blocks (NOT messages themselves) to mark cache breakpoints. Per the
/// `system_and_3` strategy, exactly 4 markers maximum per request: 1 system
/// + last 3 non-system messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    ty: String,
    ttl: String,
}

impl CacheControl {
    pub fn ephemeral(ttl: &'static str) -> Self {
        Self {
            ty: "ephemeral".to_string(),
            ttl: ttl.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AnthropicMessage {
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
        /// Phase 36.2 (D-CACHE-01): cache_control marker for system_and_3.
        /// Attached to the last content block of the last 3 non-system messages.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    Image {
        source: ImageSource, // Phase 25.1 OQ-2: multimodal user input
        /// Phase 36.2: cache_control marker. Anthropic allows on any block.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        /// Phase 36.2: cache_control marker.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
        /// Phase 36.2: cache_control marker.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
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
pub(crate) struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

// --- Response types ---

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AnthropicResponse {
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

#[derive(Debug, Clone, Default, Deserialize)]
struct AnthropicUsage {
    input_tokens: usize,
    output_tokens: usize,
    // Phase 36.2 Plan 01 deviation note: plan specified `Option<u64>` but the
    // outer `Usage` struct (ironhermes-core::types::Usage) declares these as
    // `Option<usize>`. To preserve the verbatim copy at parse sites
    // (`cache_read_input_tokens: response.usage.cache_read_input_tokens`) and
    // avoid an unnecessary `as usize` cast bleeding through every consumer,
    // these fields adopt the outer struct's `Option<usize>` shape.
    #[serde(default)]
    cache_read_input_tokens: Option<usize>,
    #[serde(default)]
    cache_creation_input_tokens: Option<usize>,
}

// =============================================================================
// SSE types for streaming
// =============================================================================

// Fields on SSE variants exist to fully deserialize the Anthropic streaming envelope;
// not all fields are read in match arms but serde requires them to parse the JSON correctly.
#[allow(dead_code)]
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

// Fields exist to fully deserialize the content-block-start envelope from Anthropic SSE;
// name/id are used in match arms; text and input are captured for completeness but not
// directly read (the delta stream carries incremental text/json via SseDelta).
#[allow(dead_code)]
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
    // Phase 36.2 Plan 01: matches outer Usage's Option<usize> cache field type
    // (see deviation note on AnthropicUsage).
    #[serde(default)]
    cache_read_input_tokens: Option<usize>,
    #[serde(default)]
    cache_creation_input_tokens: Option<usize>,
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
    if let Some(key) = config_api_key
        && !key.is_empty()
    {
        return Some(key.to_string());
    }

    // 2. Environment variable
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY")
        && !key.is_empty()
    {
        return Some(key);
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
/// - a LEADING (index 0) `system` message: becomes the system prompt (not included in messages)
/// - NON-leading `system` messages: demoted to `user` in place, content preserved
/// - `user` messages: role="user", content as text block
/// - `assistant` messages with tool_calls: content blocks (text first if any, then tool_use blocks)
/// - `tool` messages: role="user" with tool_result content block
/// - Consecutive same-role messages are merged into a single message
pub(crate) fn adapt_messages(messages: &[ChatMessage]) -> (Option<String>, Vec<AnthropicMessage>) {
    // Only a LEADING system message carries system-prompt authority.
    //
    // Non-leading `Role::System` messages are runtime-injected CONTEXT, not
    // instructions: `!` shell output and slash-command output (`tui_rata`), the
    // compression history summary (`summarizing_engine`), and advisory notes
    // (`agent_loop`). Concatenating those into `system` re-sent them as system
    // instructions on EVERY subsequent turn, permanently promoting
    // externally-sourced text (e.g. the output of `!curl`) to system authority
    // for the rest of the session.
    //
    // `client.rs::normalize_non_leading_system` already applies exactly this
    // rule on the ChatCompletions path; the native Anthropic adapter never
    // called it, so the two paths disagreed. This mirrors its semantics —
    // keep index 0 as `system`, demote every other `system` message to `user`
    // in place with its content preserved.
    let system = messages
        .first()
        .filter(|m| m.role == Role::System)
        .and_then(|m| m.content.as_ref())
        .and_then(|c| c.as_text())
        .map(String::from);

    // Convert non-system messages
    let mut raw_messages: Vec<AnthropicMessage> = Vec::new();

    for (idx, msg) in messages.iter().enumerate() {
        match msg.role {
            // The leading system message became `system` above.
            Role::System if idx == 0 => continue,
            // Every other system message is demoted to `user` in place.
            Role::System | Role::User => {
                let blocks: Vec<ContentBlock> = match msg.content.as_ref() {
                    Some(MessageContent::Text(t)) => vec![ContentBlock::Text {
                        text: t.clone(),
                        cache_control: None,
                    }],
                    Some(MessageContent::Parts(parts)) => parts
                        .iter()
                        .filter_map(|p| match p {
                            ContentPart::Text { text } => Some(ContentBlock::Text {
                                text: text.clone(),
                                cache_control: None,
                            }),
                            ContentPart::ImageUrl { image_url } => {
                                convert_image_url_to_block(&image_url.url)
                            }
                        })
                        .collect(),
                    None => vec![ContentBlock::Text {
                        text: String::new(),
                        cache_control: None,
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
                if let Some(text) = msg.content.as_ref().and_then(|c| c.as_text())
                    && !text.is_empty()
                {
                    blocks.push(ContentBlock::Text {
                        text: text.to_string(),
                        cache_control: None,
                    });
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
                            cache_control: None,
                        });
                    }
                }

                let content = if blocks.len() == 1 {
                    if let ContentBlock::Text { ref text, .. } = blocks[0] {
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
                        cache_control: None,
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
            cache_control: None,
        });
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return Some(ContentBlock::Image {
            source: ImageSource::Url {
                url: url.to_string(),
            },
            cache_control: None,
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
                    let mut blocks = vec![ContentBlock::Text {
                        text: t.clone(),
                        cache_control: None,
                    }];
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
        AnthropicContent::Text(t) => vec![ContentBlock::Text {
            text: t,
            cache_control: None,
        }],
        AnthropicContent::Blocks(blocks) => blocks,
    }
}

/// Convert OpenAI tool schemas to Anthropic tool format.
pub(crate) fn adapt_tools(tools: &[ToolSchema]) -> Vec<AnthropicTool> {
    tools
        .iter()
        .map(|t| AnthropicTool {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            input_schema: t.function.parameters.clone(),
        })
        .collect()
}

/// Phase 36.2 (D-CACHE-01/02): build an [`AnthropicRequest`] with optional
/// `cache_control` markers attached per the `system_and_3` strategy.
///
/// When `prompt_caching.enabled` is `false`, the request is byte-identical to
/// the pre-36.2 shape (system as plain string, no markers anywhere).
///
/// When enabled, markers are attached to:
/// - The system block (converted from plain string to single-element array
///   so the `cache_control` field can attach to a content block per Anthropic's API).
/// - The last content block of each of the last 3 non-system messages
///   (`saturating_sub(3)..n` range; gracefully degrades when fewer than 3 messages).
///
/// 4 markers max per request matches Anthropic's documented breakpoint cap.
pub(crate) fn build_anthropic_request(
    messages: &[ChatMessage],
    model: &str,
    max_tokens: usize,
    adapted_tools: Option<Vec<AnthropicTool>>,
    stream: Option<bool>,
    prompt_caching: &PromptCachingConfig,
) -> AnthropicRequest {
    let (system_text, mut adapted_messages) = adapt_messages(messages);

    let mut system: Option<AnthropicSystem> = system_text.map(AnthropicSystem::Text);

    if prompt_caching.enabled {
        let ttl = prompt_caching.ttl.as_anthropic_ttl();
        attach_cache_control_markers(&mut system, &mut adapted_messages, ttl);
    }

    AnthropicRequest {
        model: model.to_string(),
        messages: adapted_messages,
        system,
        max_tokens,
        tools: if adapted_tools.as_ref().map(|t| t.is_empty()).unwrap_or(true) {
            None
        } else {
            adapted_tools
        },
        stream,
    }
}

/// Phase 36.2 (D-CACHE-01): attach `cache_control` markers to the system block
/// + last 3 non-system messages (system_and_3 strategy).
///
/// **Marker count rules** (Anthropic API: max 4 breakpoints per request):
/// - 1 marker on the system block (if any).
/// - 1 marker each on the last `min(3, messages.len())` non-system messages.
/// - For multi-block messages, only the LAST content block of each marked
///   message gets the marker (cache_control attaches to the block, not the
///   message; placing on the last block caches everything up to that point).
fn attach_cache_control_markers(
    system: &mut Option<AnthropicSystem>,
    messages: &mut [AnthropicMessage],
    ttl: &'static str,
) {
    // Mark the system block — converting from Text to Blocks if needed because
    // cache_control attaches to content blocks, not bare strings.
    if let Some(s) = system.as_mut() {
        // CR-05 fix: split the early-return into per-shape arms so the
        // message-level markers below still attach when the system is
        // already Blocks. Previously the Blocks branch did `return` which
        // skipped the entire function (including the last-3-messages loop).
        match s {
            AnthropicSystem::Text(t) => {
                let text = std::mem::take(t);
                let mut block = SystemBlock::text(text);
                block.cache_control = Some(CacheControl::ephemeral(ttl));
                *s = AnthropicSystem::Blocks(vec![block]);
            }
            AnthropicSystem::Blocks(blocks) => {
                // Attach marker to the last existing system block so
                // multi-block callers still benefit from cache_control.
                if let Some(last) = blocks.last_mut() {
                    last.cache_control = Some(CacheControl::ephemeral(ttl));
                }
            }
        }
    }

    // Mark last 3 messages' last content block.
    let n = messages.len();
    let start = n.saturating_sub(3);
    for msg in &mut messages[start..n] {
        attach_marker_to_last_block(&mut msg.content, ttl);
    }
}

/// Helper: attach a cache_control marker to the last content block of a
/// message's content. If the content is plain text, convert to blocks first.
fn attach_marker_to_last_block(content: &mut AnthropicContent, ttl: &'static str) {
    // Convert Text -> Blocks so cache_control can attach to a block.
    if let AnthropicContent::Text(t) = content {
        let text = std::mem::take(t);
        *content = AnthropicContent::Blocks(vec![ContentBlock::Text {
            text,
            cache_control: None,
        }]);
    }
    if let AnthropicContent::Blocks(blocks) = content
        && let Some(last) = blocks.last_mut()
    {
        let marker = Some(CacheControl::ephemeral(ttl));
        match last {
            ContentBlock::Text { cache_control, .. }
            | ContentBlock::Image { cache_control, .. }
            | ContentBlock::ToolUse { cache_control, .. }
            | ContentBlock::ToolResult { cache_control, .. } => {
                *cache_control = marker;
            }
        }
    }
}

/// Phase 36.2 test helper: thin re-export of [`build_anthropic_request`] for
/// integration tests in `tests/prompt_cache_assertion.rs`.
///
/// Identical signature to the production builder so tests exercise the exact
/// code path.
#[doc(hidden)]
pub fn build_anthropic_request_for_test(
    messages: &[ChatMessage],
    model: &str,
    prompt_caching: &PromptCachingConfig,
) -> AnthropicRequest {
    build_anthropic_request(messages, model, 4096, None, Some(false), prompt_caching)
}

/// Phase 36.2 test helper: serialize an [`AnthropicRequest`] to JSON for
/// envelope-shape assertions.
#[doc(hidden)]
pub fn serialize_request_for_test(req: &AnthropicRequest) -> Result<String> {
    Ok(serde_json::to_string(req)?)
}

/// Convert an Anthropic response to OpenAI-compatible ChatResponse.
pub(crate) fn parse_anthropic_response(
    response: &AnthropicResponse,
) -> (ChatResponse, Option<Usage>) {
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
        cache_read_input_tokens: response.usage.cache_read_input_tokens,
        cache_creation_input_tokens: response.usage.cache_creation_input_tokens,
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
    /// Phase 36.2 (D-CACHE-02): prompt caching config.
    /// Defaults to enabled=true, ttl=1h via `PromptCachingConfig::default()`
    /// for backward compatibility with the legacy `new()` constructor.
    prompt_caching: PromptCachingConfig,
}

impl std::fmt::Debug for AnthropicClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicClient")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("default_model", &self.default_model)
            .field("prompt_caching", &self.prompt_caching)
            .finish()
    }
}

impl AnthropicClient {
    /// Construct with base_url, api_key, and default model.
    ///
    /// Creates a reqwest Client with `anthropic-version: 2023-06-01` default header
    /// and 30s connect timeout.
    ///
    /// Prompt caching defaults to enabled (TTL 1h) — call
    /// [`new_with_prompt_caching`](Self::new_with_prompt_caching) to override.
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self::new_with_prompt_caching(base_url, api_key, model, PromptCachingConfig::default())
    }

    /// Phase 36.2 (D-CACHE-02): construct with explicit prompt-caching config.
    ///
    /// Used by the AgentLoop after loading `Config::prompt_caching` to thread
    /// the operator-configured TTL + enable flag through to request building.
    pub fn new_with_prompt_caching(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        prompt_caching: PromptCachingConfig,
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
            prompt_caching,
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
        let adapted_tools = tools.map(adapt_tools);

        let request = build_anthropic_request(
            messages,
            model.unwrap_or(&self.default_model),
            max_tokens.unwrap_or(4096),
            adapted_tools,
            Some(false),
            &self.prompt_caching,
        );

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
        let adapted_tools = tools.map(adapt_tools);

        let request = build_anthropic_request(
            messages,
            model.unwrap_or(&self.default_model),
            max_tokens.unwrap_or(4096),
            adapted_tools,
            Some(true),
            &self.prompt_caching,
        );

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

                // CR-07: hard cap on SSE buffer to defend against a malicious
                // or buggy proxy that streams without ever emitting "\n\n".
                // Without this the spawned task could OOM on a slow-drip
                // attack since the 60s idle timeout fires only on read stalls.
                const MAX_SSE_BUFFER: usize = 4 * 1024 * 1024;
                if buffer.len() > MAX_SSE_BUFFER {
                    warn!(
                        "Anthropic SSE buffer exceeded {} bytes without an event boundary; aborting stream",
                        MAX_SSE_BUFFER
                    );
                    break;
                }

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

                    // CR-01: parse `data` as a JSON Value first, then inject
                    // the `type` discriminant from the SSE `event:` line. The
                    // old implementation used `format!("{{...,{}}}", &data[1..data.len()-1])`
                    // which (a) panicked on data with len < 2 (e.g., upstream
                    // sends `data: {}` or a truncated frame), (b) corrupted
                    // payloads that weren't `{...}`-shaped, and (c) injected
                    // `etype` raw into the JSON without quoting — vulnerable
                    // to malformed SSE event types containing quote bytes.
                    let inner: serde_json::Value = match serde_json::from_str(&data) {
                        Ok(v) => v,
                        Err(e) => {
                            debug!(
                                "Failed to parse Anthropic SSE data as JSON '{}': {} — data: {}",
                                etype, e, data
                            );
                            continue;
                        }
                    };
                    let mut obj = match inner {
                        serde_json::Value::Object(m) => m,
                        other => {
                            debug!(
                                "Anthropic SSE data was not a JSON object for event '{}': {}",
                                etype, other
                            );
                            continue;
                        }
                    };
                    obj.insert("type".to_string(), serde_json::Value::String(etype.clone()));
                    let parsed: AnthropicSseEvent =
                        match serde_json::from_value(serde_json::Value::Object(obj)) {
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
                                        cache_read_input_tokens: u.cache_read_input_tokens,
                                        cache_creation_input_tokens: u.cache_creation_input_tokens,
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

    /// Phase 36.6.4 code review CR-02 (Critical): a NON-leading `Role::System`
    /// message must never reach the `system` field.
    ///
    /// `tui_rata::App::apply_shell_outcome` pushes `!` shell output into history
    /// as `Role::System`. Before this fix, `adapt_messages` filtered EVERY
    /// system message regardless of position and joined them into `system`,
    /// which Anthropic re-sends on every turn — so the output of `!curl` gained
    /// system-prompt authority permanently. `client.rs::normalize_non_leading_system`
    /// already prevented this on the ChatCompletions path; the two adapters
    /// disagreed.
    ///
    /// FAILS ON THE PRE-FIX TREE: `system` was
    /// `"You are a helpful assistant.\n\nuid=0(root) gid=0(root)"`.
    #[test]
    fn non_leading_system_is_demoted_to_user_not_hoisted_into_system_prompt() {
        let mut shell_output = ChatMessage::user("uid=0(root) gid=0(root)");
        shell_output.role = Role::System;

        let messages = vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::user("run id for me"),
            shell_output,
            ChatMessage::user("what did that say?"),
        ];
        let (system, msgs) = adapt_messages(&messages);

        // The leading system message — and ONLY it — is the system prompt.
        assert_eq!(
            system.as_deref(),
            Some("You are a helpful assistant."),
            "only the leading system message may become the system prompt; \
             shell output must never be appended to it"
        );
        assert!(
            !system.as_deref().unwrap_or("").contains("uid=0"),
            "externally-sourced shell output leaked into the system prompt, \
             where Anthropic re-sends it with system authority every turn"
        );

        // The demoted message survives as ordinary user context. All three
        // remaining messages are user-role and merge into one.
        assert_eq!(msgs.len(), 1, "consecutive user messages merge");
        assert_eq!(msgs[0].role, "user");
        let AnthropicContent::Blocks(blocks) = &msgs[0].content else {
            panic!("expected content blocks for the merged user message");
        };
        let joined: String = blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("uid=0(root)"),
            "demoting must PRESERVE the content as user context, not drop it; got: {joined}"
        );
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
                matches!(&blocks[0], ContentBlock::Text { text, .. } if text == "Hi");
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
                    ContentBlock::ToolUse {
                        id, name, input, ..
                    } => {
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
                ..Default::default()
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
                ..Default::default()
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
                assert!(
                    matches!(&blocks[0], ContentBlock::Text { text, .. } if text == "Let me help")
                );
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

    // =========================================================================
    // Phase 36.2 Plan 01: AnthropicUsage cache field deserialization tests
    //
    // Verifies AnthropicUsage round-trips `cache_read_input_tokens` and
    // `cache_creation_input_tokens` from Anthropic response bodies, including
    // the absent-field default-to-None path (#[serde(default)] semantics).
    // =========================================================================

    #[test]
    fn anthropic_usage_deserializes_cache_fields_when_present() {
        // Test 1: Both cache fields present — they round-trip into the struct.
        let json = r#"{
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_read_input_tokens": 8200,
            "cache_creation_input_tokens": 4200
        }"#;
        let usage: AnthropicUsage = serde_json::from_str(json).expect("parse");
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_input_tokens, Some(8200));
        assert_eq!(usage.cache_creation_input_tokens, Some(4200));
    }

    #[test]
    fn anthropic_usage_defaults_cache_fields_to_none_when_absent() {
        // Test 2: Legacy / non-cached response — fields absent → None via #[serde(default)].
        let json = r#"{
            "input_tokens": 100,
            "output_tokens": 50
        }"#;
        let usage: AnthropicUsage = serde_json::from_str(json).expect("parse");
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_input_tokens, None);
        assert_eq!(usage.cache_creation_input_tokens, None);
    }

    #[test]
    fn anthropic_usage_treats_null_cache_fields_as_none() {
        // Test 3: Explicit JSON null for cache fields → Option semantics preserve None.
        let json = r#"{
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_read_input_tokens": null,
            "cache_creation_input_tokens": null
        }"#;
        let usage: AnthropicUsage = serde_json::from_str(json).expect("parse");
        assert_eq!(usage.cache_read_input_tokens, None);
        assert_eq!(usage.cache_creation_input_tokens, None);
    }

    #[test]
    fn parse_anthropic_response_populates_outer_usage_cache_fields() {
        // Verifies the parse-site fix at lines ~537-538: outer Usage wrapper now
        // copies cache fields through from AnthropicUsage instead of hardcoding None.
        let response = AnthropicResponse {
            id: "msg_cache".to_string(),
            content: vec![ResponseContentBlock::Text {
                text: "cached!".to_string(),
            }],
            model: "claude-opus-4-7".to_string(),
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 12,
                output_tokens: 7,
                cache_read_input_tokens: Some(8200),
                cache_creation_input_tokens: Some(4200),
            },
        };
        let (_chat, usage) = parse_anthropic_response(&response);
        let u = usage.expect("usage populated");
        assert_eq!(u.prompt_tokens, 12);
        assert_eq!(u.completion_tokens, 7);
        assert_eq!(u.cache_read_input_tokens, Some(8200));
        assert_eq!(u.cache_creation_input_tokens, Some(4200));
    }

    /// INV-36.2-CR-01: regression guard against the SSE format-splice
    /// panic. The pre-fix code in chat_completion_stream built the tagged
    /// JSON via `format!("{{\"type\":\"{etype}\",{}}}", &data[1..data.len()-1])`
    /// which panicked when `data.len() < 2` (e.g., a transport hiccup truncates
    /// to a single brace) and could corrupt non-object payloads. The fix
    /// parses `data` as a `serde_json::Value` first and inserts the `type`
    /// key only if the value is an object. This guard fails if anyone
    /// re-introduces the splice pattern.
    #[test]
    fn inv_36_2_cr_01_sse_parse_does_not_use_format_splice() {
        const SOURCE: &str = include_str!("anthropic_client.rs");
        let non_comment: String = SOURCE
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        // The dangerous pattern that produced CR-01: indexing into `data` with
        // `data.len() - 1` could underflow on short payloads. Detect any
        // reappearance. Build the needle via concat! so this assertion's own
        // text doesn't trip the contains() check above.
        let needle = concat!("&data[1..data", ".len() - 1]");
        assert!(
            !non_comment.contains(needle),
            "Phase 36.2 CR-01: anthropic_client.rs must not slice `data` with \
             the underflow-prone `len minus one` index — that pattern panics on \
             short SSE payloads. Use serde_json::from_str(&data) to parse, then \
             inject the type key."
        );
        assert!(
            non_comment.contains("serde_json::from_value(serde_json::Value::Object(obj))"),
            "Phase 36.2 CR-01: SSE parse must go through serde_json::from_value \
             on a Value::Object after injecting the type discriminant."
        );
    }
}

//! Phase 46.2: `CodexResponses` backend — `providers.merge`
//! (`https://api-gateway.merge.dev/v1`), `POST /responses`.
//!
//! REWORK (2026-07-04, D-07 live UAT): merge's `POST /v1/responses` is NOT the
//! OpenAI Responses API. Its `input[]`/response/SSE are ANTHROPIC-MESSAGES-flavored
//! (content-block arrays, `tool_result`/`tool_use` blocks, `input_tokens`/
//! `output_tokens` usage, Anthropic-style named SSE events) merely wrapped in a
//! `/responses` envelope. The original OpenAI-Responses build (`function_call` /
//! `function_call_output` items, `instructions`/`store`/`max_output_tokens`,
//! `response.*` dotted SSE events) 422'd on any tool-history turn. This module is
//! now built to merge's real schema, mirroring `anthropic_client.rs`.
//!
//! Request conversion (D-02): `ChatMessage[]` -> `CodexRequest`. Every `system`
//! message becomes a `MessageInput{role:"system"}` (merge accepts role system —
//! no `instructions` field exists). Assistant `tool_calls` -> a `message` item
//! whose `content` is a block array of an optional `{type:"text"}` block followed
//! by one `{type:"tool_use", id, name, input:<object>}` block per call. `tool`
//! messages -> a top-level `{type:"tool_result", tool_use_id, content}` item.
//! `extra` is flattened onto the top-level body so `project_id` +
//! `include_routing_metadata` reach the wire. There is NO `instructions`, NO
//! `store`; the token cap is `max_tokens` (NOT `max_output_tokens`).
//!
//! Response parsing (D-03) + SSE (D-04) — CORRECTED 2026-07-05 after capturing
//! merge's real stream live (the 46.2 rework guessed an Anthropic-SSE shape merge
//! never sends, so the parser dropped every frame -> empty "turn-ended-empty"
//! turns). merge's real envelope is NEITHER OpenAI-Responses NOR Anthropic SSE:
//!   `{ object, output: [ { finish_reason, content: [{type:"text"|"tool_use"}] } ], usage }`
//! Non-streaming responses use `object:"response"`. STREAMING uses `data:`-only
//! SSE frames (NO `event:` line) that are CUMULATIVE snapshots — each frame
//! repeats the full `output[].content` so far — discriminated by
//! `object` = "response.stream" (in-progress) | "response.done" (terminal, with
//! `finish_reason` + `usage`). `process_codex_frame` diffs text into suffix
//! ContentDeltas and de-dups tool_use blocks by id (emitting one full
//! id+name+arguments ToolCallDelta each). A deserialize-miss on the non-streaming
//! body or a non-2xx response still surfaces the raw body (truncated ~512 chars).
//!
//! (`CodexClient` HTTP wrapper + the streaming frame parser are also in this file.)

use ironhermes_core::{
    ChatChoice, ChatMessage, ChatResponse, FunctionCall, MessageContent, Role, ToolCall,
    ToolSchema, Usage,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::client::StreamEvent;

// =============================================================================
// Request-side serde types (D-02) — merge's Anthropic-flavored `/responses` body
// =============================================================================

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CodexRequest {
    pub(crate) model: String,
    pub(crate) input: Vec<CodexInputItem>,
    /// merge uses `max_tokens` (NOT OpenAI Responses' `max_output_tokens`).
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<CodexTool>>,
    /// D-02: MUST flatten (unlike `anthropic_client`'s dropped `_extra`) so
    /// `project_id` + `include_routing_metadata` reach the wire.
    #[serde(flatten)]
    extra: HashMap<String, serde_json::Value>,
}

/// merge `input[]` discriminated union (tag `type`) — only two top-level variants.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CodexInputItem {
    /// `{type:"message", role, content: string | ContentBlock[]}`
    Message {
        role: String,
        content: CodexMessageContent,
    },
    /// `{type:"tool_result", tool_use_id, content: string | ContentBlock[]}` —
    /// a TOP-LEVEL input item (NOT nested inside a message), keyed by
    /// `tool_use_id` (== `ToolCall.id`, D-05), NOT `call_id`.
    ToolResult {
        tool_use_id: String,
        content: CodexMessageContent,
    },
}

/// A message/tool_result `content` field: either a plain string (text-only) or a
/// block array (when carrying `tool_use` blocks). `#[serde(untagged)]` keeps
/// text messages a bare JSON string and block messages a JSON array.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub(crate) enum CodexMessageContent {
    Text(String),
    Blocks(Vec<CodexContentBlock>),
}

/// ContentBlock union (tag `type`) used inside `content` arrays. We only ever
/// EMIT `text` + `tool_use` (merge also defines image/document/audio/thinking).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CodexContentBlock {
    Text {
        text: String,
    },
    /// `input` is a JSON OBJECT (not an arguments string, unlike OpenAI).
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CodexTool {
    #[serde(rename = "type")]
    ty: &'static str, // "function"
    name: String,
    description: String,
    parameters: serde_json::Value,
    // NOTE: flat — no nested `function: {...}` wrapper, unlike Chat Completions' ToolSchema.
}

// =============================================================================
// Response-side serde types (D-03) — merge's REAL `/responses` envelope,
// captured live 2026-07-05 (D-07 follow-up; merge's openapi left it undefined):
//   { object, output: [ { finish_reason, content: [ {type,...} ] } ], usage }
// The SAME envelope is used by the non-streaming response (object:"response")
// and by every streaming frame (object:"response.stream" | "response.done").
// Tolerant serde everywhere (no deny_unknown_fields; #[serde(other)] catch-all).
// =============================================================================

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CodexResponseBody {
    /// merge nests content under `output[]` — NOT a top-level `content` field.
    #[serde(default)]
    output: Vec<CodexOutputItem>,
    #[serde(default)]
    pub(crate) usage: Option<CodexUsage>,
}

/// One entry in merge's `output[]` — the assistant message: its `content`
/// blocks plus the terminal `finish_reason` ("stop" | "tool_use" | ...).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CodexOutputItem {
    #[serde(default)]
    finish_reason: Option<String>,
    #[serde(default)]
    content: Vec<CodexResponseContentBlock>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CodexResponseContentBlock {
    Text {
        #[serde(default)]
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    // Catch-all so an unmodeled block type (thinking/image/etc.) never crashes
    // the whole content[] deserialize.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CodexUsage {
    #[serde(default)]
    input_tokens: usize,
    #[serde(default)]
    output_tokens: usize,
    #[serde(default)]
    cache_read_input_tokens: Option<usize>,
    #[serde(default)]
    cache_creation_input_tokens: Option<usize>,
}

// =============================================================================
// Streaming frame (D-04) — merge streams `data:`-only SSE frames (NO `event:`
// line). Each frame is a CUMULATIVE snapshot of the response envelope above
// (content grows each frame; it is NOT sent as incremental deltas),
// discriminated by `object`:
//   "response.stream" — in-progress (content accumulating; usage null)
//   "response.done"   — terminal   (finish_reason + usage populated)
// =============================================================================

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CodexStreamFrame {
    #[serde(default)]
    object: String,
    #[serde(default)]
    output: Vec<CodexOutputItem>,
    #[serde(default)]
    usage: Option<CodexUsage>,
    /// Present only on error frames. merge's streaming-error envelope is
    /// undocumented; surfaced verbatim as a ProviderError (fail to fallback).
    #[serde(default)]
    error: Option<serde_json::Value>,
}

// =============================================================================
// Conversion functions (pure, zero-network — unit-tested below)
// =============================================================================

/// Convert messages into merge `input[]` items (D-02): text -> `message`
/// (role system/user/assistant); assistant tool_calls -> a `message` whose
/// content is a `[text?, tool_use...]` block array; tool results -> a top-level
/// `tool_result` item keyed by `tool_use_id`.
fn to_input_items(messages: &[ChatMessage]) -> Vec<CodexInputItem> {
    let mut items = Vec::new();
    for m in messages {
        match m.role {
            Role::Tool => {
                // D-05: tool_use_id == the message's tool_call_id (== ToolCall.id).
                let tool_use_id = m.tool_call_id.clone().unwrap_or_default();
                let content = m.content_text().unwrap_or("").to_string();
                items.push(CodexInputItem::ToolResult {
                    tool_use_id,
                    content: CodexMessageContent::Text(content),
                });
            }
            Role::System | Role::User | Role::Assistant => {
                let role_str = match m.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => unreachable!("Role::Tool handled above"),
                }
                .to_string();

                let has_tool_calls = m.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty());

                if has_tool_calls {
                    // Assistant carrying tool_calls -> block array: optional text
                    // block first, then one tool_use block per call.
                    let mut blocks: Vec<CodexContentBlock> = Vec::new();
                    if let Some(text) = m.content_text()
                        && !text.is_empty()
                    {
                        blocks.push(CodexContentBlock::Text {
                            text: text.to_string(),
                        });
                    }
                    for tc in m.tool_calls.as_ref().expect("has_tool_calls checked") {
                        let input = parse_tool_arguments(&tc.function.arguments);
                        blocks.push(CodexContentBlock::ToolUse {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            input,
                        });
                    }
                    items.push(CodexInputItem::Message {
                        role: role_str,
                        content: CodexMessageContent::Blocks(blocks),
                    });
                } else if let Some(text) = m.content_text() {
                    items.push(CodexInputItem::Message {
                        role: role_str,
                        content: CodexMessageContent::Text(text.to_string()),
                    });
                }
            }
        }
    }
    items
}

/// Minimal user turn appended by [`ensure_trailing_user`] when a converted
/// `input[]` would otherwise end on a system/assistant message.
const CODEX_CONTINUATION_NUDGE: &str = "Please continue.";

/// Enforce Anthropic's request-shape invariant on merge's `/responses` input:
/// the conversation MUST end with a `user` turn. Anthropic (via merge, e.g.
/// `anthropic/claude-sonnet-5`) rejects a request whose messages end on a
/// non-user turn with `invalid_request_error: "This model does not support
/// assistant message prefill. The conversation must end with a user message."`,
/// which the streaming layer surfaces as a fatal "Codex Responses streaming
/// failed (SSE error)" 400.
///
/// This happens after a context-compression pass: `summarizing_engine::compress`
/// can leave the message list ending on the pinned `[CONTEXT HISTORY]`
/// `Role::System` summary (once the recent tail is pruned) — or on an assistant
/// turn — and `to_input_items` passes system messages through inline, so the
/// converted `input[]` ends on a non-user item. A trailing `tool_result` is
/// already a user turn in Anthropic's shape, so it needs no fix; only a trailing
/// system/assistant `message` is appended to. In a well-formed agent loop the
/// request always ends with a user or tool_result turn, so this only fires on
/// the post-compression edge case.
///
/// NOTE: the native `anthropic_client` path has the same latent gap — tracked as
/// a follow-up in `.planning/todos/pending/2026-07-07-anthropic-family-trailing-user-guard.md`.
fn ensure_trailing_user(input: &mut Vec<CodexInputItem>) {
    let ends_with_user_turn = match input.last() {
        // A `tool_result` maps to a `user`-role turn in Anthropic's Messages shape.
        Some(CodexInputItem::ToolResult { .. }) => true,
        Some(CodexInputItem::Message { role, .. }) => role == "user",
        // Empty input is degenerate (nothing to prefill-continue) — leave as-is.
        None => true,
    };
    if !ends_with_user_turn {
        input.push(CodexInputItem::Message {
            role: "user".to_string(),
            content: CodexMessageContent::Text(CODEX_CONTINUATION_NUDGE.to_string()),
        });
    }
}

/// Parse a `ToolCall.function.arguments` JSON string into an object `Value`.
/// Empty or unparseable arguments fall back to `{}` (merge requires an object).
fn parse_tool_arguments(arguments: &str) -> serde_json::Value {
    if arguments.trim().is_empty() {
        return serde_json::json!({});
    }
    serde_json::from_str(arguments).unwrap_or_else(|_| serde_json::json!({}))
}

/// Convert OpenAI-compat tool schemas to flat merge tools (D-02 — no nested
/// `function: {...}` wrapper; identical shape merge already accepts).
fn to_codex_tools(tools: &[ToolSchema]) -> Vec<CodexTool> {
    tools
        .iter()
        .map(|t| CodexTool {
            ty: "function",
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            parameters: t.function.parameters.clone(),
        })
        .collect()
}

/// Build a `CodexRequest` from ironhermes' internal chat types (D-02).
///
/// `max_tokens` defaults to 4096 when `None` (mirrors `anthropic_client`).
/// `extra` is flattened, never dropped. No `instructions`, no `store`.
pub(crate) fn build_codex_request(
    messages: &[ChatMessage],
    tools: Option<&[ToolSchema]>,
    model: &str,
    max_tokens: Option<usize>,
    temperature: Option<f64>,
    stream: Option<bool>,
    extra: HashMap<String, serde_json::Value>,
) -> CodexRequest {
    let mut input = to_input_items(messages);
    // Anthropic (via merge) requires the request to end with a user turn — see
    // ensure_trailing_user. After compression the list can end on the
    // [CONTEXT HISTORY] system summary or an assistant turn, which 400s.
    ensure_trailing_user(&mut input);
    let codex_tools = tools.map(to_codex_tools).filter(|t| !t.is_empty());

    CodexRequest {
        model: model.to_string(),
        input,
        max_tokens: Some(max_tokens.unwrap_or(4096)),
        temperature,
        stream,
        tools: codex_tools,
        extra,
    }
}

/// Parse a `CodexResponseBody` (Anthropic Messages shape) into
/// `(ChatResponse, Option<Usage>)` (D-03).
///
/// `finish_reason` is `tool_calls` when any `tool_use` block is present, else `stop`.
pub(crate) fn parse_codex_response(body: &CodexResponseBody) -> (ChatResponse, Option<Usage>) {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    for item in &body.output {
        for block in &item.content {
            match block {
                CodexResponseContentBlock::Text { text } => {
                    text_parts.push(text.clone());
                }
                CodexResponseContentBlock::ToolUse { id, name, input } => {
                    // merge sends `input` as a JSON object; serialize back to the
                    // arguments string ironhermes' ToolCall expects.
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
                CodexResponseContentBlock::Unknown => {}
            }
        }
    }

    let content_text = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join(""))
    };
    let has_tool_calls = !tool_calls.is_empty();
    let tool_calls_opt = if has_tool_calls {
        Some(tool_calls)
    } else {
        None
    };

    let message = ChatMessage {
        role: Role::Assistant,
        content: content_text.map(MessageContent::Text),
        tool_calls: tool_calls_opt,
        tool_call_id: None,
        name: None,
        is_recall_context: false,
    };

    let finish_reason = if has_tool_calls { "tool_calls" } else { "stop" }.to_string();

    let chat_response = ChatResponse {
        id: String::new(),
        object: "response".to_string(),
        created: 0,
        model: String::new(),
        choices: vec![ChatChoice {
            index: 0,
            message,
            finish_reason: Some(finish_reason),
        }],
        usage: None, // filled by the caller
    };

    let usage = body.usage.as_ref().map(codex_usage_to_usage);

    (chat_response, usage)
}

pub(crate) fn codex_usage_to_usage(u: &CodexUsage) -> Usage {
    Usage {
        prompt_tokens: u.input_tokens,
        completion_tokens: u.output_tokens,
        total_tokens: u.input_tokens + u.output_tokens,
        cache_read_input_tokens: u.cache_read_input_tokens,
        cache_creation_input_tokens: u.cache_creation_input_tokens,
    }
}

/// Streaming accumulator for merge's cumulative snapshot frames (D-04). Each
/// frame repeats the full content-so-far AND builds tool-call `input`
/// incrementally (early frames carry `input:{}`), so we buffer the latest
/// tool-call snapshot and emit it only at finalization, when `input` is complete.
#[derive(Default)]
pub(crate) struct CodexStreamState {
    emitted_text_len: usize,
    /// Latest snapshot of (id, name, input) for the turn's tool calls.
    tool_calls: Vec<(String, String, serde_json::Value)>,
    usage: Option<Usage>,
    finish_reason: Option<String>,
}

/// Normalize merge's `finish_reason` to ironhermes' convention: "tool_use" ->
/// "tool_calls" (matches `parse_codex_response`); other reasons pass through.
pub(crate) fn normalize_codex_finish_reason(reason: Option<String>) -> Option<String> {
    reason.map(|r| {
        if r == "tool_use" {
            "tool_calls".to_string()
        } else {
            r
        }
    })
}

/// Emit the buffered tool calls (each once, with COMPLETE args), then Usage,
/// then Done. Called exactly once at stream end (terminal "response.done"
/// frame, `[DONE]` sentinel, or clean EOF). Deferring tool-call emission to here
/// is deliberate: merge streams a tool call's `input` incrementally across
/// cumulative frames, so only the final snapshot has complete arguments.
pub(crate) fn finalize_codex_stream(state: &mut CodexStreamState) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    for (index, (id, name, input)) in state.tool_calls.drain(..).enumerate() {
        let arguments = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
        events.push(StreamEvent::ToolCallDelta {
            index,
            id: Some(id),
            name: Some(name),
            arguments: Some(arguments),
        });
    }
    if let Some(u) = state.usage.take() {
        events.push(StreamEvent::Usage(u));
    }
    events.push(StreamEvent::Done(normalize_codex_finish_reason(
        state.finish_reason.take(),
    )));
    events
}

/// Map one merge stream frame (a cumulative snapshot) to the NEW `StreamEvent`s
/// it produces, updating `state`. Text is emitted as the suffix beyond what has
/// already been sent; tool calls are emitted exactly once each (de-duped by id,
/// full id+name+arguments in a single delta — the aggregator appends args and
/// sets id/name). Returns `(events, is_terminal)`; the terminal
/// "response.done" frame appends `Usage` (if any) then `Done`.
pub(crate) fn process_codex_frame(
    frame: &CodexStreamFrame,
    state: &mut CodexStreamState,
) -> (Vec<StreamEvent>, bool) {
    let mut events = Vec::new();

    // Best-effort error frame -> ProviderError (activates fallback_providers).
    if let Some(err) = &frame.error {
        let raw = serde_json::to_string(err).unwrap_or_default();
        let code = err.get("code").and_then(|c| {
            c.as_str()
                .map(str::to_string)
                .or_else(|| c.as_u64().map(|n| n.to_string()))
        });
        let message = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("stream error");
        events.push(StreamEvent::ProviderError(codex_sse_error_to_bail_string(
            &raw,
            code.as_deref(),
            message,
        )));
        return (events, true);
    }

    // Walk the cumulative snapshot: concatenate text, and capture the CURRENT
    // tool-call blocks (merge fills their `input` over successive frames).
    let mut full_text = String::new();
    let mut tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();
    for item in &frame.output {
        for block in &item.content {
            match block {
                CodexResponseContentBlock::Text { text } => full_text.push_str(text),
                CodexResponseContentBlock::ToolUse { id, name, input } => {
                    if !id.is_empty() {
                        tool_calls.push((id.clone(), name.clone(), input.clone()));
                    }
                }
                CodexResponseContentBlock::Unknown => {}
            }
        }
        if item.finish_reason.is_some() {
            state.finish_reason = item.finish_reason.clone();
        }
    }

    // Emit only the newly-appended text (frames are cumulative, so `full_text`
    // is a superstring of what we've already sent). The char-boundary guard is
    // defensive against any non-prefix growth (would otherwise panic on slice).
    if full_text.len() > state.emitted_text_len
        && full_text.is_char_boundary(state.emitted_text_len)
    {
        events.push(StreamEvent::ContentDelta(
            full_text[state.emitted_text_len..].to_string(),
        ));
        state.emitted_text_len = full_text.len();
    }

    // Buffer the newest non-empty tool-call snapshot; a tool call's `input` is
    // only guaranteed complete on the terminal frame (merge streams it
    // incrementally — early frames carry `input:{}`), so emission is deferred to
    // finalize_codex_stream. Emitting on first sight would run the tool with
    // empty args ("Missing required parameter" — D-07b).
    if !tool_calls.is_empty() {
        state.tool_calls = tool_calls;
    }

    if let Some(u) = &frame.usage {
        state.usage = Some(codex_usage_to_usage(u));
    }

    let is_terminal = frame.object == "response.done" || frame.object == "response";
    if is_terminal {
        events.extend(finalize_codex_stream(state));
    }
    (events, is_terminal)
}

/// Truncate a raw body to ~512 chars for diagnostics, on a UTF-8 char boundary
/// (never panics on multi-byte input).
fn body_excerpt(raw: &str) -> String {
    match raw.char_indices().nth(512) {
        Some((idx, _)) => format!("{}... [truncated]", &raw[..idx]),
        None => raw.to_string(),
    }
}

/// Mirror of `client.rs::sse_error_to_bail_string` (private there, so mirrored
/// rather than made `pub(crate)`). Caps the raw frame body at 512 chars.
fn codex_sse_error_to_bail_string(raw: &str, code: Option<&str>, message: &str) -> String {
    let status_token = match code {
        Some("400") => "400 Bad Request",
        Some("401") => "401 Unauthorized",
        Some("403") => "403 Forbidden",
        Some("404") => "404 Not Found",
        Some("422") => "422 Unprocessable Entity",
        Some("429") => "429 Too Many Requests",
        Some("500") => "500 Internal Server Error",
        Some("502") => "502 Bad Gateway",
        Some("503") => "503 Service Unavailable",
        Some("504") => "504 Gateway Timeout",
        _ => "SSE error",
    };
    format!(
        "Codex Responses streaming failed ({}): {} [body: {}]",
        status_token,
        message,
        body_excerpt(raw)
    )
}

// =============================================================================
// CodexClient
// =============================================================================

/// HTTP client for `providers.merge`'s `/responses` endpoint.
///
/// The Debug impl redacts `api_key` (Pitfall 9, mirrors `anthropic_client.rs`).
#[derive(Clone)]
pub struct CodexClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    default_model: String,
}

impl std::fmt::Debug for CodexClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexClient")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("default_model", &self.default_model)
            .finish()
    }
}

impl CodexClient {
    /// Construct with base_url, api_key, and default model. 30s connect timeout
    /// (mirrors `LlmClient`/`AnthropicClient`).
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            default_model: model.into(),
        }
    }

    /// Non-streaming chat completion. Same signature as `LlmClient`/`AnthropicClient`
    /// for `AnyClient` dispatch.
    pub async fn chat_completion(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSchema]>,
        model: Option<&str>,
        max_tokens: Option<usize>,
        temperature: Option<f64>,
        extra: Option<HashMap<String, serde_json::Value>>,
    ) -> anyhow::Result<ChatResponse> {
        // D-05: pre-send pairing guard, exactly as both existing backends do.
        if let Err(diag) = ironhermes_core::validate_tool_call_pairing(messages) {
            tracing::warn!(
                diag = %diag,
                "tool-call pairing invariant violated before non-streaming Codex send"
            );
            anyhow::bail!("tool-call pairing invariant violated: {}", diag);
        }

        let resolved_model = model.unwrap_or(&self.default_model).to_string();
        let request = build_codex_request(
            messages,
            tools,
            &resolved_model,
            max_tokens,
            temperature,
            Some(false),
            extra.unwrap_or_default(),
        );

        // base_url already ends in /v1 — do NOT re-append it.
        let url = format!("{}/responses", self.base_url);
        tracing::debug!(url = %url, model = %request.model, "Sending Codex Responses chat completion request");

        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                anyhow::anyhow!("Failed to send Codex Responses chat completion request: {e}")
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            // D-07 de-risk: non-2xx already surfaces the raw body (truncated).
            anyhow::bail!(
                "Codex Responses chat completion failed ({}): {}",
                status,
                body_excerpt(&body)
            );
        }

        // D-07 de-risk: read the body as text first so a deserialize-miss can
        // surface the raw body (truncated) — the inferred Anthropic shape is not
        // in merge's openapi, so any deviation must be a one-shot re-UAT fix.
        let raw = response.text().await.map_err(|e| {
            anyhow::anyhow!("Failed to read Codex Responses chat completion body: {e}")
        })?;
        let response_body: CodexResponseBody = serde_json::from_str(&raw).map_err(|e| {
            anyhow::anyhow!(
                "Failed to parse Codex Responses chat completion response: {e} [body: {}]",
                body_excerpt(&raw)
            )
        })?;

        tracing::debug!(
            finish_reason = ?response_body
                .output
                .first()
                .and_then(|o| o.finish_reason.as_deref()),
            "Codex Responses chat completion response received"
        );

        let (mut chat_response, usage) = parse_codex_response(&response_body);
        chat_response.model = resolved_model;
        chat_response.usage = usage;
        Ok(chat_response)
    }

    /// Streaming chat completion. Returns a channel receiver for StreamEvents —
    /// same return type as `LlmClient`/`AnthropicClient`.
    pub async fn chat_completion_stream(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSchema]>,
        model: Option<&str>,
        max_tokens: Option<usize>,
        temperature: Option<f64>,
        extra: Option<HashMap<String, serde_json::Value>>,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        // D-05: pre-send pairing guard, exactly as both existing backends do.
        if let Err(diag) = ironhermes_core::validate_tool_call_pairing(messages) {
            tracing::warn!(
                diag = %diag,
                "tool-call pairing invariant violated before streaming Codex send"
            );
            anyhow::bail!("tool-call pairing invariant violated: {}", diag);
        }

        let resolved_model = model.unwrap_or(&self.default_model).to_string();
        let request = build_codex_request(
            messages,
            tools,
            &resolved_model,
            max_tokens,
            temperature,
            Some(true),
            extra.unwrap_or_default(),
        );

        // base_url already ends in /v1 — do NOT re-append it.
        let url = format!("{}/responses", self.base_url);
        tracing::debug!(url = %url, model = %request.model, "Sending Codex Responses streaming request");

        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                anyhow::anyhow!("Failed to send Codex Responses streaming request: {e}")
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Codex Responses streaming request failed ({}): {}",
                status,
                body_excerpt(&body)
            );
        }

        let (tx, rx) = tokio::sync::mpsc::channel(256);

        tokio::spawn(async move {
            use futures::StreamExt;

            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();
            let chunk_timeout = tokio::time::Duration::from_secs(60);

            // merge streams cumulative snapshots (D-04): each frame carries the
            // full content so far. `state` diffs text and de-dups tool calls by
            // id across frames; the terminal "response.done" frame carries usage.
            let mut state = CodexStreamState::default();

            loop {
                let chunk_result =
                    match tokio::time::timeout(chunk_timeout, byte_stream.next()).await {
                        Ok(Some(result)) => result,
                        Ok(None) => break,
                        Err(_) => {
                            tracing::warn!("Codex Responses SSE stream read timed out after 60s");
                            break;
                        }
                    };

                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("Codex Responses stream error: {}", e);
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // T-46.2-02: hard cap on SSE buffer (same defense as the other
                // two backends' CR-07 fix) against a slow-drip/unbounded stream.
                const MAX_SSE_BUFFER: usize = 4 * 1024 * 1024;
                if buffer.len() > MAX_SSE_BUFFER {
                    tracing::warn!(
                        "Codex Responses SSE buffer exceeded {} bytes without an event boundary; aborting stream",
                        MAX_SSE_BUFFER
                    );
                    break;
                }

                while let Some(event_end) = buffer.find("\n\n") {
                    let event_block = buffer[..event_end].to_string();
                    buffer = buffer[event_end + 2..].to_string();

                    // merge frames are `data:`-only (NO `event:` line). Per the
                    // SSE spec, multiple `data:` lines in one event join with
                    // '\n'; comment/other lines are ignored.
                    let mut data = String::new();
                    for line in event_block.lines() {
                        if let Some(d) = line.strip_prefix("data:") {
                            if !data.is_empty() {
                                data.push('\n');
                            }
                            data.push_str(d.strip_prefix(' ').unwrap_or(d));
                        }
                    }
                    if data.is_empty() {
                        continue;
                    }
                    // Optional OpenAI-style terminal sentinel — finalize cleanly.
                    if data == "[DONE]" {
                        for ev in finalize_codex_stream(&mut state) {
                            if tx.send(ev).await.is_err() {
                                return;
                            }
                        }
                        return;
                    }

                    let frame: CodexStreamFrame = match serde_json::from_str(&data) {
                        Ok(f) => f,
                        Err(e) => {
                            tracing::debug!(
                                "Failed to parse Codex Responses stream frame: {e} — data: {}",
                                body_excerpt(&data)
                            );
                            continue;
                        }
                    };

                    let (events, is_terminal) = process_codex_frame(&frame, &mut state);
                    for ev in events {
                        let is_error = matches!(ev, StreamEvent::ProviderError(_));
                        if tx.send(ev).await.is_err() {
                            return;
                        }
                        if is_error {
                            return;
                        }
                    }
                    if is_terminal {
                        return;
                    }
                }
            }

            // Stream ended without an explicit "response.done"/[DONE] frame
            // (clean EOF, timeout, or read error): finalize with whatever we
            // accumulated (incl. tool calls with complete args) so the agent
            // loop gets a proper finish rather than a bare channel-close.
            for ev in finalize_codex_stream(&mut state) {
                if tx.send(ev).await.is_err() {
                    break;
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
    use ironhermes_core::{FunctionSchema, ToolCall};

    fn make_tool_schema(name: &str, description: &str) -> ToolSchema {
        ToolSchema {
            schema_type: "function".to_string(),
            function: FunctionSchema {
                name: name.to_string(),
                description: description.to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": { "input": {"type": "string"} }
                }),
            },
        }
    }

    // ---- build_codex_request shape (merge Anthropic-flavored) ----

    #[test]
    fn build_codex_request_shape_and_extras_flatten() {
        let tool_calls = vec![ToolCall {
            id: "c1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "get_weather".to_string(),
                arguments: r#"{"city":"SF"}"#.to_string(),
            },
        }];
        let messages = vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::user("What's the weather?"),
            ChatMessage {
                role: Role::Assistant,
                content: None,
                tool_calls: Some(tool_calls),
                tool_call_id: None,
                name: None,
                is_recall_context: false,
            },
            ChatMessage::tool_result("c1", "72F sunny"),
        ];

        let mut extra = HashMap::new();
        extra.insert("project_id".to_string(), serde_json::json!("85c3d7ba-test"));
        extra.insert(
            "include_routing_metadata".to_string(),
            serde_json::json!(true),
        );

        let request = build_codex_request(
            &messages,
            None,
            "anthropic/claude-fable-5",
            None,
            Some(0.7),
            Some(false),
            extra,
        );

        let value = serde_json::to_value(&request).expect("serialize");

        assert_eq!(value["model"], "anthropic/claude-fable-5");
        assert_eq!(value["temperature"], 0.7);
        assert_eq!(value["max_tokens"], 4096);
        assert_eq!(value["project_id"], "85c3d7ba-test");
        assert_eq!(value["include_routing_metadata"], true);

        // No `store`, no `instructions`, no `max_output_tokens`.
        assert!(value.get("store").is_none(), "no store field");
        assert!(value.get("instructions").is_none(), "no instructions field");
        assert!(
            value.get("max_output_tokens").is_none(),
            "merge uses max_tokens, not max_output_tokens"
        );

        let input = value["input"].as_array().expect("input array");
        // system message + user message + assistant(tool_use) message + tool_result
        assert_eq!(input.len(), 4);

        // [0] system message, string content.
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "system");
        assert_eq!(input[0]["content"], "You are a helpful assistant.");

        // [1] user message, string content.
        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["role"], "user");
        assert_eq!(input[1]["content"], "What's the weather?");

        // [2] assistant message with a tool_use block array (parsed object input).
        assert_eq!(input[2]["type"], "message");
        assert_eq!(input[2]["role"], "assistant");
        let blocks = input[2]["content"].as_array().expect("block array");
        assert_eq!(
            blocks.len(),
            1,
            "no text on this assistant msg, one tool_use"
        );
        assert_eq!(blocks[0]["type"], "tool_use");
        assert_eq!(blocks[0]["id"], "c1");
        assert_eq!(blocks[0]["name"], "get_weather");
        assert_eq!(
            blocks[0]["input"],
            serde_json::json!({"city": "SF"}),
            "input must be a parsed JSON object, not an arguments string"
        );

        // [3] tool result — top-level tool_result item keyed by tool_use_id.
        assert_eq!(input[3]["type"], "tool_result");
        assert_eq!(input[3]["tool_use_id"], "c1");
        assert_eq!(input[3]["content"], "72F sunny");
    }

    #[test]
    fn build_codex_request_assistant_text_and_tool_call_emits_text_then_tool_use() {
        let tool_calls = vec![ToolCall {
            id: "call_x".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "do_thing".to_string(),
                arguments: String::new(), // empty -> {} fallback
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
        let request = build_codex_request(
            &messages,
            None,
            "test-model",
            None,
            None,
            Some(false),
            HashMap::new(),
        );
        let value = serde_json::to_value(&request).expect("serialize");
        let blocks = value["input"][0]["content"]
            .as_array()
            .expect("block array");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "Let me help");
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(
            blocks[1]["input"],
            serde_json::json!({}),
            "empty/invalid arguments fall back to an empty object"
        );
    }

    #[test]
    fn build_codex_request_system_message_stays_role_system() {
        // merge accepts role:"system" as a message item — no instructions demotion.
        let messages = vec![
            ChatMessage::user("hi"),
            ChatMessage::system("[CONTEXT] compressed history"),
            ChatMessage::assistant("ok"),
        ];
        let request = build_codex_request(
            &messages,
            None,
            "test-model",
            None,
            None,
            Some(false),
            HashMap::new(),
        );
        let value = serde_json::to_value(&request).expect("serialize");
        let input = value["input"].as_array().expect("input array");
        // 3 source messages + 1 appended user-continuation: this fixture ends on
        // an assistant turn, which Anthropic rejects as a prefill, so
        // ensure_trailing_user appends a trailing user item.
        assert_eq!(input.len(), 4);
        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["role"], "system");
        assert_eq!(input[1]["content"], "[CONTEXT] compressed history");
        assert_eq!(
            input[3]["role"], "user",
            "guard appends a trailing user turn"
        );
    }

    // ---- ensure_trailing_user guard (Anthropic "must end with user") ----

    #[test]
    fn build_codex_request_appends_user_when_trailing_system_history() {
        // Reproduces the live 400: after compression the message list ends on the
        // pinned [CONTEXT HISTORY] Role::System summary. Anthropic rejects a
        // request that doesn't end with a user turn ("assistant message prefill …
        // must end with a user message"), so the guard must append one.
        let messages = vec![
            ChatMessage::system("You are helpful."),
            ChatMessage::user("original question"),
            ChatMessage::assistant("working on it"),
            ChatMessage::system("[CONTEXT HISTORY] summarized older turns"),
        ];
        let request = build_codex_request(
            &messages,
            None,
            "anthropic/claude-sonnet-5",
            None,
            None,
            Some(true),
            HashMap::new(),
        );
        let value = serde_json::to_value(&request).expect("serialize");
        let input = value["input"].as_array().expect("input array");
        assert_eq!(input.len(), 5, "one user-continuation item appended");
        let last = input.last().expect("non-empty input");
        assert_eq!(last["type"], "message");
        assert_eq!(last["role"], "user", "request must end with a user turn");
        assert_eq!(last["content"], "Please continue.");
    }

    #[test]
    fn build_codex_request_appends_user_when_trailing_assistant() {
        let messages = vec![
            ChatMessage::user("hi"),
            ChatMessage::assistant("here is my reply"),
        ];
        let request = build_codex_request(
            &messages,
            None,
            "anthropic/claude-sonnet-5",
            None,
            None,
            Some(false),
            HashMap::new(),
        );
        let value = serde_json::to_value(&request).expect("serialize");
        let input = value["input"].as_array().expect("input array");
        assert_eq!(
            input.len(),
            3,
            "trailing assistant -> user-continuation appended"
        );
        assert_eq!(input.last().expect("non-empty input")["role"], "user");
    }

    #[test]
    fn build_codex_request_no_append_when_trailing_user() {
        let messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("the current question"),
        ];
        let request = build_codex_request(
            &messages,
            None,
            "anthropic/claude-sonnet-5",
            None,
            None,
            Some(false),
            HashMap::new(),
        );
        let value = serde_json::to_value(&request).expect("serialize");
        let input = value["input"].as_array().expect("input array");
        assert_eq!(input.len(), 2, "already ends with user — no guard append");
        assert_eq!(
            input.last().expect("non-empty input")["content"],
            "the current question"
        );
    }

    #[test]
    fn build_codex_request_no_append_when_trailing_tool_result() {
        // A tool_result is a user-role turn in Anthropic's shape — already valid.
        let tool_calls = vec![ToolCall {
            id: "c1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "get_x".to_string(),
                arguments: "{}".to_string(),
            },
        }];
        let messages = vec![
            ChatMessage::user("do it"),
            ChatMessage {
                role: Role::Assistant,
                content: None,
                tool_calls: Some(tool_calls),
                tool_call_id: None,
                name: None,
                is_recall_context: false,
            },
            ChatMessage::tool_result("c1", "result payload"),
        ];
        let request = build_codex_request(
            &messages,
            None,
            "anthropic/claude-sonnet-5",
            None,
            None,
            Some(false),
            HashMap::new(),
        );
        let value = serde_json::to_value(&request).expect("serialize");
        let input = value["input"].as_array().expect("input array");
        assert_eq!(
            input.len(),
            3,
            "trailing tool_result is a user turn — no guard append"
        );
        assert_eq!(
            input.last().expect("non-empty input")["type"],
            "tool_result"
        );
    }

    #[test]
    fn build_codex_request_flat_tool_schema_no_nested_function_wrapper() {
        let messages = vec![ChatMessage::user("hi")];
        let tools = vec![make_tool_schema("search", "Search the web")];
        let request = build_codex_request(
            &messages,
            Some(&tools),
            "test-model",
            None,
            None,
            Some(false),
            HashMap::new(),
        );
        let value = serde_json::to_value(&request).expect("serialize");
        let tool = &value["tools"][0];
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["name"], "search");
        assert!(
            tool.get("function").is_none(),
            "must be flat, no nested function wrapper"
        );
    }

    // ---- parse_codex_response (merge output[]-nested envelope) ----

    #[test]
    fn parse_codex_response_extracts_text_and_tool_calls() {
        // merge nests content under output[]; finish_reason lives on the item.
        let json = serde_json::json!({
            "id": "resp_1",
            "object": "response",
            "output": [{
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "finish_reason": "tool_use",
                "content": [
                    { "type": "text", "text": "The weather is sunny." },
                    {
                        "type": "tool_use", "id": "call_abc",
                        "name": "get_weather", "input": { "city": "SF" }
                    }
                ]
            }],
            "usage": {
                "input_tokens": 17,
                "output_tokens": 20,
                "cache_read_input_tokens": 4
            }
        });
        let body: CodexResponseBody = serde_json::from_value(json).expect("parse");

        let (chat_response, usage) = parse_codex_response(&body);
        let choice = &chat_response.choices[0];
        assert_eq!(choice.message.content_text(), Some("The weather is sunny."));
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));

        let tool_calls = choice
            .message
            .tool_calls
            .as_ref()
            .expect("tool_calls present");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_abc");
        assert_eq!(tool_calls[0].function.name, "get_weather");
        // input object serialized back to an arguments string.
        let args: serde_json::Value =
            serde_json::from_str(&tool_calls[0].function.arguments).unwrap();
        assert_eq!(args["city"], "SF");

        let u = usage.expect("usage populated");
        assert_eq!(u.prompt_tokens, 17);
        assert_eq!(u.completion_tokens, 20);
        assert_eq!(u.total_tokens, 37);
        assert_eq!(u.cache_read_input_tokens, Some(4));
    }

    #[test]
    fn parse_codex_response_text_only_finish_reason_stop() {
        let json = serde_json::json!({
            "output": [{ "finish_reason": "end_turn",
                         "content": [{ "type": "text", "text": "Hi there." }] }],
            "usage": { "input_tokens": 5, "output_tokens": 3 }
        });
        let body: CodexResponseBody = serde_json::from_value(json).expect("parse");
        let (chat_response, _usage) = parse_codex_response(&body);
        let choice = &chat_response.choices[0];
        assert_eq!(choice.finish_reason.as_deref(), Some("stop"));
        assert!(choice.message.tool_calls.is_none());
        assert_eq!(choice.message.content_text(), Some("Hi there."));
    }

    #[test]
    fn parse_codex_response_tolerates_unknown_block_type() {
        // An unmodeled block type (e.g. "thinking") must not crash the parse.
        let json = serde_json::json!({
            "output": [{ "content": [
                { "type": "thinking", "thinking": "hmm" },
                { "type": "text", "text": "answer" }
            ]}],
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        });
        let body: CodexResponseBody =
            serde_json::from_value(json).expect("must tolerate unknown block type");
        let (chat_response, _usage) = parse_codex_response(&body);
        assert_eq!(
            chat_response.choices[0].message.content_text(),
            Some("answer")
        );
    }

    // ---- process_codex_frame (merge cumulative `data:`-only snapshots) ----

    fn frame(v: serde_json::Value) -> CodexStreamFrame {
        serde_json::from_value(v).expect("frame parse")
    }

    /// Regression for the actual delivery bug: merge frames have NO `event:`
    /// line — a bare `data:` JSON payload must deserialize into a frame. The
    /// pre-fix parser required an `event:` line and dropped every frame.
    #[test]
    fn process_codex_frame_bare_data_no_event_line_parses() {
        let f: CodexStreamFrame = serde_json::from_str(
            r#"{"object":"response.done","output":[{"finish_reason":"stop","content":[{"type":"text","text":"hi"}]}],"usage":{"input_tokens":1,"output_tokens":1}}"#,
        )
        .expect("bare merge frame (no event: line) must parse");
        let mut state = CodexStreamState::default();
        let (events, done) = process_codex_frame(&f, &mut state);
        assert!(done);
        assert!(events.iter().any(|e| matches!(e, StreamEvent::Done(_))));
    }

    /// Cumulative text snapshots must emit only the NEW suffix per frame (not the
    /// whole accumulated text), then Usage + Done on the terminal frame.
    #[test]
    fn process_codex_frame_text_snapshots_emit_suffix_deltas_then_done() {
        let mut state = CodexStreamState::default();

        let (e1, done1) = process_codex_frame(
            &frame(serde_json::json!({
                "object": "response.stream",
                "output": [{ "content": [{ "type": "text", "text": "Hello" }] }]
            })),
            &mut state,
        );
        assert!(!done1);
        assert_eq!(e1.len(), 1);
        match &e1[0] {
            StreamEvent::ContentDelta(t) => assert_eq!(t, "Hello"),
            other => panic!("expected ContentDelta, got {other:?}"),
        }

        let (e2, done2) = process_codex_frame(
            &frame(serde_json::json!({
                "object": "response.stream",
                "output": [{ "content": [{ "type": "text", "text": "Hello world" }] }]
            })),
            &mut state,
        );
        assert!(!done2);
        assert_eq!(e2.len(), 1);
        match &e2[0] {
            StreamEvent::ContentDelta(t) => assert_eq!(t, " world"),
            other => panic!("expected suffix ContentDelta, got {other:?}"),
        }

        let (e3, done3) = process_codex_frame(
            &frame(serde_json::json!({
                "object": "response.done",
                "output": [{ "finish_reason": "stop",
                             "content": [{ "type": "text", "text": "Hello world" }] }],
                "usage": { "input_tokens": 3, "output_tokens": 2 }
            })),
            &mut state,
        );
        assert!(done3);
        // No new text, then Usage + Done("stop").
        assert_eq!(e3.len(), 2);
        assert!(matches!(&e3[0], StreamEvent::Usage(u) if u.completion_tokens == 2));
        assert!(matches!(&e3[1], StreamEvent::Done(Some(r)) if r == "stop"));
    }

    /// Regression (D-07b): merge builds a tool call's `input` incrementally
    /// across cumulative frames (early frames carry `input:{}`). The tool call
    /// must be emitted ONCE, at finalization, with the COMPLETE args from the
    /// final snapshot — never the empty first one (which ran the tool with no
    /// args -> "Missing required parameter"). finish_reason "tool_use"
    /// normalizes to "tool_calls".
    #[test]
    fn process_codex_frame_tool_input_completed_at_finalization() {
        let mut state = CodexStreamState::default();

        // Frame 1: tool_use present, `input` still empty -> nothing emitted yet.
        let (e1, done1) = process_codex_frame(
            &frame(serde_json::json!({
                "object": "response.stream",
                "output": [{ "content": [{
                    "type": "tool_use", "id": "toolu_1", "name": "web_read", "input": {}
                }]}]
            })),
            &mut state,
        );
        assert!(!done1);
        assert!(e1.is_empty(), "must NOT emit while input is empty: {e1:?}");

        // Frame 2: `input` now filled in -> still buffered, not emitted.
        let (e2, done2) = process_codex_frame(
            &frame(serde_json::json!({
                "object": "response.stream",
                "output": [{ "content": [{
                    "type": "tool_use", "id": "toolu_1", "name": "web_read",
                    "input": { "url": "https://example.com/x" }
                }]}]
            })),
            &mut state,
        );
        assert!(!done2);
        assert!(e2.is_empty(), "buffer until terminal frame: {e2:?}");

        // Terminal: emit the ONE tool call with COMPLETE args, then Usage + Done.
        let (e3, done3) = process_codex_frame(
            &frame(serde_json::json!({
                "object": "response.done",
                "output": [{ "finish_reason": "tool_use", "content": [{
                    "type": "tool_use", "id": "toolu_1", "name": "web_read",
                    "input": { "url": "https://example.com/x" }
                }]}],
                "usage": { "input_tokens": 10, "output_tokens": 5 }
            })),
            &mut state,
        );
        assert!(done3);
        assert_eq!(e3.len(), 3);
        match &e3[0] {
            StreamEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments,
            } => {
                assert_eq!(*index, 0);
                assert_eq!(id.as_deref(), Some("toolu_1"));
                assert_eq!(name.as_deref(), Some("web_read"));
                let args: serde_json::Value =
                    serde_json::from_str(arguments.as_deref().unwrap()).unwrap();
                assert_eq!(
                    args["url"], "https://example.com/x",
                    "tool call must carry the COMPLETE input from the final snapshot"
                );
            }
            other => panic!("expected complete ToolCallDelta, got {other:?}"),
        }
        assert!(matches!(&e3[1], StreamEvent::Usage(_)));
        assert!(
            matches!(&e3[2], StreamEvent::Done(Some(r)) if r == "tool_calls"),
            "finish_reason tool_use must normalize to tool_calls: {:?}",
            e3[2]
        );
    }
}

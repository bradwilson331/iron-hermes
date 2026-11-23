use anyhow::{Context, Result};
use futures::StreamExt;
use ironhermes_core::{
    ChatMessage, ChatRequest, ChatResponse, ChatStreamChunk, FunctionCall, Role, ToolCall,
    ToolSchema, Usage,
};
use reqwest::Client;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use tracing::{debug, warn};

/// Events emitted during streaming.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A text content delta.
    ContentDelta(String),
    /// A tool call is being built (index, partial data).
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    },
    /// Usage statistics (sent at end of stream).
    Usage(Usage),
    /// Stream finished with a reason.
    Done(Option<String>),
    /// Provider returned an error payload inside the SSE stream at HTTP 200. The inner String is a
    /// pre-formatted error message in the '(NNN Reason): message [body: ...]' style for direct use
    /// with anyhow!. See phase 36.14-sse-stream-error-fallback-gap.
    ProviderError(String),
}

/// Extract the HTTP-like status code from an SSE error envelope value.
///
/// Reads `val.pointer("/error/code")` first, falls back to `val.get("code")`.
/// Accepts BOTH numeric (`as_u64`) AND string (`as_str().parse::<u16>()`) shapes —
/// addresses Codex MEDIUM #2 (provider variation: OpenRouter may return "code":400
/// while some upstreams forward "code":"400").
fn extract_sse_error_code(val: &serde_json::Value) -> Option<u16> {
    let code_val = val.pointer("/error/code").or_else(|| val.get("code"))?;
    // Try numeric first
    if let Some(n) = code_val.as_u64().and_then(|n| u16::try_from(n).ok()) {
        return Some(n);
    }
    // Try string-shaped (e.g. "400")
    code_val.as_str().and_then(|s| s.parse::<u16>().ok())
}

/// Format a bail string from an SSE error envelope.
///
/// Extracts the `error.code` (numeric or string), maps it to a reason text,
/// extracts the `error.message` field, and caps the raw JSON tail at 512 chars
/// with a `... [truncated]` marker to prevent provider request IDs / echoed user
/// input from bloating warn!-level fallback logs (Codex MEDIUM #3).
///
/// Output format: `"Streaming chat completion failed (NNN Reason): {message} [body: {raw}]"`
fn sse_error_to_bail_string(raw: &str, val: &serde_json::Value) -> String {
    let code = extract_sse_error_code(val);
    let (status_token, _reason) = match code {
        Some(400) => ("400 Bad Request", "Bad Request"),
        Some(401) => ("401 Unauthorized", "Unauthorized"),
        Some(403) => ("403 Forbidden", "Forbidden"),
        Some(404) => ("404 Not Found", "Not Found"),
        Some(429) => ("429 Too Many Requests", "Too Many Requests"),
        Some(500) => ("500 Internal Server Error", "Internal Server Error"),
        Some(502) => ("502 Bad Gateway", "Bad Gateway"),
        Some(503) => ("503 Service Unavailable", "Service Unavailable"),
        Some(504) => ("504 Gateway Timeout", "Gateway Timeout"),
        _ => ("SSE error", "SSE error"),
    };
    let message = val
        .pointer("/error/message")
        .and_then(|m| m.as_str())
        .unwrap_or("<no message>");
    let body_excerpt = if raw.len() <= 512 {
        raw.to_string()
    } else {
        format!("{}... [truncated]", &raw[..512])
    };
    format!(
        "Streaming chat completion failed ({}): {} [body: {}]",
        status_token, message, body_excerpt
    )
}

/// Cause C fix: syntactic model-ID sanity guard.
///
/// Returns `Some(reason)` when the model ID is structurally malformed — i.e.
/// guaranteed to be rejected by any provider — so the caller can bail before
/// the streaming POST rather than paying a round-trip HTTP 400 + silent failover.
///
/// Rejects: empty IDs, IDs containing ASCII whitespace, IDs containing control
/// characters. These are unambiguous corruption signals that no real provider
/// model ID exhibits.
///
/// Does NOT attempt to catch single-character-insertion typos (e.g. "g0emma",
/// "1qwen3.7-max") — those are syntactically valid strings indistinguishable from
/// real model IDs without a model list. Catching them would require a models-cache
/// lookup on the hot streaming path (disk I/O per request; false-rejects on stale
/// cache). The syntactic guard handles structural garbage; the typo class remains
/// best-addressed upstream at model-selection time.
fn model_id_looks_malformed(model: &str) -> Option<&'static str> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return Some("model id is empty");
    }
    if trimmed.chars().any(|c| c.is_ascii_whitespace()) {
        return Some("model id contains whitespace");
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Some("model id contains control characters");
    }
    None
}

/// Anthropic-family models (reachable in ChatCompletions mode via OpenRouter →
/// Vertex/Bedrock/Anthropic) reject a `system` message that is not the leading
/// message. OpenAI tolerates mid-array system messages, but ironhermes injects
/// several of them (the compression history summary in `summarizing_engine`,
/// plus transient/advisory notes in `agent_loop`), so a long-context turn that
/// has compressed fails on Anthropic providers with
/// `messages.N: role 'system' must follow a 'user'/'assistant' message`.
///
/// Normalize before the wire: keep only a leading (index 0) `system` message as
/// `system`; demote every other `system` message to `user` in place (content,
/// name, and position preserved). Zero-copy when nothing needs changing, so the
/// common (uncompressed) turn and OpenAI-only turns pay no allocation.
///
/// Also replaces tool-call `arguments` that are not valid JSON with `{}`.
/// llama-server re-parses every replayed tool call when it renders the chat
/// template, so one truncated call from the model (e.g. `{"query": "Qwen2`)
/// made every later request in the session fail with HTTP 500. History keeps
/// the raw text; the tool result already carries the parse error for the model.
fn normalize_non_leading_system(messages: &[ChatMessage]) -> std::borrow::Cow<'_, [ChatMessage]> {
    let bad_args = |m: &ChatMessage| {
        m.tool_calls.as_ref().is_some_and(|calls| {
            calls
                .iter()
                .any(|c| serde_json::from_str::<serde_json::Value>(&c.function.arguments).is_err())
        })
    };
    let needs_fix = messages
        .iter()
        .enumerate()
        .any(|(i, m)| (i != 0 && m.role == Role::System) || bad_args(m));
    if !needs_fix {
        return std::borrow::Cow::Borrowed(messages);
    }
    let fixed = messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut m = m.clone();
            if i != 0 && m.role == Role::System {
                m.role = Role::User;
            }
            if let Some(calls) = m.tool_calls.as_mut() {
                for c in calls.iter_mut() {
                    if serde_json::from_str::<serde_json::Value>(&c.function.arguments).is_err() {
                        c.function.arguments = "{}".to_string();
                    }
                }
            }
            m
        })
        .collect();
    std::borrow::Cow::Owned(fixed)
}

#[cfg(test)]
mod system_norm_tests {
    use super::*;

    #[test]
    fn invalid_tool_call_arguments_become_empty_object_on_wire() {
        let call = |args: &str| ToolCall {
            id: "c1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "web_search".to_string(),
                arguments: args.to_string(),
            },
        };
        let msgs = vec![
            ChatMessage::system("sys"),
            ChatMessage::assistant_tool_calls(vec![
                call(r#"{"query": "Qwen2"#),
                call(r#"{"q":1}"#),
            ]),
        ];
        let out = normalize_non_leading_system(&msgs);
        let calls = out[1].tool_calls.as_ref().unwrap();
        assert_eq!(calls[0].function.arguments, "{}");
        assert_eq!(
            calls[1].function.arguments, r#"{"q":1}"#,
            "valid args untouched"
        );
        // History itself is not rewritten.
        assert_eq!(
            msgs[1].tool_calls.as_ref().unwrap()[0].function.arguments,
            r#"{"query": "Qwen2"#
        );
    }

    #[test]
    fn demotes_non_leading_system_to_user() {
        let msgs = vec![
            ChatMessage::system("main prompt"),
            ChatMessage::user("hi"),
            ChatMessage::system("[CONTEXT HISTORY] summary"),
            ChatMessage::assistant("ok"),
        ];
        let out = normalize_non_leading_system(&msgs);
        assert_eq!(out[0].role, Role::System, "leading system preserved");
        assert_eq!(out[1].role, Role::User);
        assert_eq!(out[2].role, Role::User, "mid-array system demoted to user");
        assert_eq!(out[3].role, Role::Assistant);
        assert!(
            out[2]
                .content_text()
                .map(|t| t.contains("[CONTEXT HISTORY]"))
                .unwrap_or(false),
            "content preserved on demotion"
        );
    }

    #[test]
    fn zero_copy_when_only_leading_system() {
        let msgs = vec![ChatMessage::system("sys"), ChatMessage::user("q")];
        let out = normalize_non_leading_system(&msgs);
        assert!(
            matches!(out, std::borrow::Cow::Borrowed(_)),
            "no non-leading system -> borrow, no clone"
        );
        assert_eq!(out[0].role, Role::System);
        assert_eq!(out[1].role, Role::User);
    }
}

/// Client for OpenAI-compatible chat completions API.
#[derive(Clone)]
pub struct LlmClient {
    http: Client,
    base_url: String,
    api_key: String,
    default_model: String,
    /// Phase 36.2 CR-09: provider name (e.g., "openrouter") used to detect
    /// OpenRouter Claude routing in the streaming path. Empty string means
    /// "unknown" — equivalent to the pre-CR-09 behavior (no special routing).
    provider_name: String,
    /// Phase 36.2 CR-09: prompt-caching config. When `enabled` is true AND
    /// the (provider, model) pair is OpenRouter Claude, the streaming path
    /// routes through `build_openrouter_chat_request` so `cache_control`
    /// markers attach per the system_and_3 strategy.
    prompt_caching: ironhermes_core::config::PromptCachingConfig,
}

impl LlmClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            http: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            default_model: model.into(),
            provider_name: String::new(),
            prompt_caching: ironhermes_core::config::PromptCachingConfig::default(),
        }
    }

    /// Phase 36.2 CR-09: set the provider name. Required for OpenRouter Claude
    /// routing detection (`is_openrouter_claude(provider, model)`). Empty
    /// string disables the special routing.
    pub fn set_provider_name(&mut self, name: impl Into<String>) {
        self.provider_name = name.into();
    }

    /// Phase 36.2 CR-09: set the prompt-caching config. When `enabled` AND
    /// the client's provider+model are OpenRouter Claude, the streaming
    /// request body is built via `build_openrouter_chat_request` so
    /// `cache_control` markers attach per the system_and_3 strategy.
    pub fn set_prompt_caching(&mut self, cfg: ironhermes_core::config::PromptCachingConfig) {
        self.prompt_caching = cfg;
    }

    /// `chat_completion` with D-09 learn-shrink-retry around a context-length
    /// rejection (auxiliary callers; see `aux_fit`).
    pub async fn chat_completion_fitting(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSchema]>,
        model: Option<&str>,
        max_tokens: Option<usize>,
        temperature: Option<f64>,
        extra: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<ChatResponse> {
        crate::aux_fit::chat_with_context_recovery(
            self.base_url(),
            model.unwrap_or_else(|| self.model()),
            messages,
            max_tokens,
            |m, mt| {
                let extra = extra.clone();
                async move {
                    self.chat_completion(&m, tools, model, mt, temperature, extra)
                        .await
                }
            },
        )
        .await
    }

    /// Non-streaming chat completion.
    pub async fn chat_completion(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSchema]>,
        model: Option<&str>,
        max_tokens: Option<usize>,
        temperature: Option<f64>,
        extra: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<ChatResponse> {
        // Demote non-leading `system` messages to `user` so Anthropic-family
        // providers don't reject a mid-array system message (see
        // normalize_non_leading_system). Shadows `messages` for all uses below.
        let normalized_msgs = normalize_non_leading_system(messages);
        let messages: &[ChatMessage] = &normalized_msgs;
        // Phase 25.1 GAP-7: pre-send invariant guard. Convert opaque provider
        // 400 ("tool_call_ids did not have response messages") into a
        // deterministic, locally-named, non-retriable error before the wire.
        if let Err(diag) = ironhermes_core::validate_tool_call_pairing(messages) {
            warn!(
                diag = %diag,
                "tool-call pairing invariant violated before non-streaming send"
            );
            anyhow::bail!("tool-call pairing invariant violated: {}", diag);
        }
        let request = ChatRequest {
            model: model.unwrap_or(&self.default_model).to_string(),
            messages: messages.to_vec(),
            tools: tools.map(|t| t.to_vec()),
            max_tokens,
            temperature,
            stream: Some(false),
            stop: None,
            extra: extra.unwrap_or_default(),
        };

        let url = format!("{}/chat/completions", self.base_url);
        debug!(url = %url, model = %request.model, "Sending chat completion request");

        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send chat completion request")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Chat completion failed ({}): {}", status, body);
        }

        let chat_response: ChatResponse = response
            .json()
            .await
            .context("Failed to parse chat completion response")?;

        debug!(
            model = %chat_response.model,
            choices = chat_response.choices.len(),
            "Chat completion response received"
        );

        Ok(chat_response)
    }

    /// Streaming chat completion. Returns a channel receiver for stream events.
    pub async fn chat_completion_stream(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSchema]>,
        model: Option<&str>,
        max_tokens: Option<usize>,
        temperature: Option<f64>,
        extra: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        // Demote non-leading `system` messages to `user` so Anthropic-family
        // providers don't reject a mid-array system message (see
        // normalize_non_leading_system). Shadows `messages` for all uses below.
        let normalized_msgs = normalize_non_leading_system(messages);
        let messages: &[ChatMessage] = &normalized_msgs;
        // Phase 25.1 GAP-7: pre-send invariant guard. Catches orphan tool-call
        // sequences (caused by same-tick timestamp ties in session restore)
        // BEFORE the streaming POST. Returns a non-retriable error so the
        // agent_loop retry path treats it as terminal — no 400 cascade.
        if let Err(diag) = ironhermes_core::validate_tool_call_pairing(messages) {
            warn!(
                diag = %diag,
                "tool-call pairing invariant violated before streaming send"
            );
            anyhow::bail!("tool-call pairing invariant violated: {}", diag);
        }
        // Phase 36.2 Plan 07 fix: OpenAI-compat providers (OpenRouter, vLLM,
        // Ollama ≥ 0.5, etc.) only return per-stream `usage` when the request
        // body sets `stream_options.include_usage = true`. Without this, the
        // streaming response omits `usage`, `StreamEvent::Usage` never fires,
        // and `agent_loop::write_usage_success` writes usage_events rows with
        // all-zero tokens — breaking /usage rollups and the Plan 10 status
        // pills. Caller-supplied `extra` wins (lets a test override the flag).
        let mut extra = extra.unwrap_or_default();
        extra
            .entry("stream_options".to_string())
            .or_insert_with(|| serde_json::json!({ "include_usage": true }));

        let resolved_model = model.unwrap_or(&self.default_model).to_string();

        // Cause C fix: reject structurally-malformed model IDs before the POST
        // so we avoid paying a round-trip HTTP 400 + silent failover. The guard
        // is syntactic-only (no disk I/O, no models-cache.json on the hot path).
        // Tradeoff: single-character-insertion typos (e.g. "g0emma", "1qwen3.7-max")
        // are indistinguishable from valid-looking IDs without a model list and are
        // NOT caught here — those remain best-addressed upstream at model-selection
        // time or via a future opt-in cache check. See DIAGNOSIS-primary-llm-failover.md.
        if let Some(reason) = model_id_looks_malformed(&resolved_model) {
            warn!(model = %resolved_model, reason, "rejecting malformed model id before streaming send");
            anyhow::bail!("invalid model id '{}': {}", resolved_model, reason);
        }

        let url = format!("{}/chat/completions", self.base_url);

        // Phase 36.2 CR-09: when the (provider, model) pair is OpenRouter
        // Claude AND prompt_caching is enabled, route through the OpenRouter
        // Claude request builder so `cache_control` markers attach per the
        // system_and_3 strategy. Pre-fix this builder existed and was tested
        // but no production code path invoked it — the OpenRouter Claude
        // cache hits Plan 11 was meant to deliver never fired.
        let response =
            if crate::any_client::is_openrouter_claude(&self.provider_name, &resolved_model)
                && self.prompt_caching.enabled
            {
                let or_request = crate::any_client::build_openrouter_chat_request_full(
                    &self.provider_name,
                    &resolved_model,
                    messages,
                    tools,
                    max_tokens,
                    temperature,
                    Some(true),
                    &self.prompt_caching,
                    extra,
                );
                debug!(
                    url = %url,
                    model = %or_request.model,
                    "Sending streaming OpenRouter Claude request (cache_control attached)"
                );
                self.http
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .header("Content-Type", "application/json")
                    .json(&or_request)
                    .send()
                    .await
                    .context("Failed to send streaming request")?
            } else {
                let request = ChatRequest {
                    model: resolved_model,
                    messages: messages.to_vec(),
                    tools: tools.map(|t| t.to_vec()),
                    max_tokens,
                    temperature,
                    stream: Some(true),
                    stop: None,
                    extra,
                };

                debug!(url = %url, model = %request.model, "Sending streaming LLM request");
                self.http
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .header("Content-Type", "application/json")
                    .json(&request)
                    .send()
                    .await
                    .context("Failed to send streaming request")?
            };

        let status = response.status();
        debug!(status = %status, "LLM response received");
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Streaming chat completion failed ({}): {}", status, body);
        }

        let (tx, rx) = mpsc::channel(256);

        tokio::spawn(async move {
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();
            let chunk_timeout = Duration::from_secs(60);
            // Phase 36.2 Plan 07 fix: defer emitting `StreamEvent::Done` until
            // the `[DONE]` sentinel arrives (or the byte stream ends), so the
            // post-finish_reason `usage` chunk that OpenAI-spec providers send
            // when `stream_options.include_usage: true` is honored is captured
            // first. Previously we emitted Done immediately on finish_reason
            // and `return`ed — the consumer broke on Done and the trailing
            // usage chunk was lost, leaving every `usage_events` row at zero
            // tokens and stranding the TUI / web token pills at 0/128k.
            let mut pending_finish_reason: Option<String> = None;

            loop {
                let chunk_result = match timeout(chunk_timeout, byte_stream.next()).await {
                    Ok(Some(result)) => result,
                    Ok(None) => break, // stream ended
                    Err(_) => {
                        warn!("SSE stream read timed out after 60s");
                        break;
                    }
                };

                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("Stream error: {}", e);
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // CR-07: hard cap on SSE buffer to defend against a malicious
                // or buggy proxy that streams without ever emitting newlines.
                // Without this the spawned task could OOM on a slow-drip
                // attack since the 60s idle timeout fires only on read stalls.
                const MAX_SSE_BUFFER: usize = 4 * 1024 * 1024;
                if buffer.len() > MAX_SSE_BUFFER {
                    warn!(
                        "SSE buffer exceeded {} bytes without a line terminator; aborting stream",
                        MAX_SSE_BUFFER
                    );
                    break;
                }

                // Process complete SSE lines
                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    let data = if let Some(stripped) = line.strip_prefix("data: ") {
                        stripped.trim()
                    } else {
                        continue;
                    };

                    if data == "[DONE]" {
                        // Emit Done last so any usage chunk delivered between
                        // the finish_reason chunk and [DONE] has already been
                        // forwarded to the consumer via StreamEvent::Usage.
                        let _ = tx
                            .send(StreamEvent::Done(pending_finish_reason.take()))
                            .await;
                        return;
                    }

                    match serde_json::from_str::<ChatStreamChunk>(data) {
                        Ok(chunk) => {
                            // Process usage first (may appear in same chunk as finish_reason).
                            if let Some(ref usage) = chunk.usage {
                                let _ = tx.send(StreamEvent::Usage(usage.clone())).await;
                            }
                            for choice in &chunk.choices {
                                if let Some(ref content) = choice.delta.content {
                                    let _ =
                                        tx.send(StreamEvent::ContentDelta(content.clone())).await;
                                }
                                if let Some(ref tool_calls) = choice.delta.tool_calls {
                                    for tc in tool_calls {
                                        let _ = tx
                                            .send(StreamEvent::ToolCallDelta {
                                                index: tc.index,
                                                id: tc.id.clone(),
                                                name: tc
                                                    .function
                                                    .as_ref()
                                                    .and_then(|f| f.name.clone()),
                                                arguments: tc
                                                    .function
                                                    .as_ref()
                                                    .and_then(|f| f.arguments.clone()),
                                            })
                                            .await;
                                    }
                                }
                                if let Some(ref reason) = choice.finish_reason {
                                    // Stash the reason; the Done event is
                                    // emitted only when `[DONE]` arrives or
                                    // the byte stream ends — see the local
                                    // declaration above for rationale.
                                    pending_finish_reason = Some(reason.clone());
                                }
                            }
                        }
                        Err(_parse_err) => {
                            // D-01 (phase 36.14): second-pass — is this an SSE provider error envelope?
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(data)
                                && val.get("error").is_some()
                            {
                                let msg = sse_error_to_bail_string(data, &val);
                                let _ = tx.send(StreamEvent::ProviderError(msg)).await;
                                return;
                            }
                            // D-05 (phase 36.14): not a recognised error envelope — preserve existing debug log.
                            debug!(
                                "Failed to parse stream chunk: {} — data: {}",
                                _parse_err, data
                            );
                        }
                    }
                }
            }
            // Stream ended without an explicit `[DONE]` sentinel (server closed
            // the connection, read timeout, or transport error). Forward the
            // stashed finish_reason so the consumer's `Done` arm fires once;
            // dropping `tx` afterwards closes the channel naturally.
            let _ = tx
                .send(StreamEvent::Done(pending_finish_reason.take()))
                .await;
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

/// A streaming tool call delta: (index, id, name, arguments).
pub type ToolCallDelta = (usize, Option<String>, Option<String>, Option<String>);

/// Per-index accumulator for `assemble_tool_calls_from_stream`. The two extra
/// fields (`delta_count`, `id_ever_seen`) exist purely for the empty-id
/// diagnostic below — they add no allocation and are never part of the
/// returned `ToolCall`.
#[derive(Default)]
struct ToolCallAccum {
    id: String,
    name: String,
    arguments: String,
    delta_count: u32,
    id_ever_seen: bool,
}

/// Assemble a complete ChatMessage from streaming tool call deltas.
pub fn assemble_tool_calls_from_stream(deltas: &[ToolCallDelta]) -> Vec<ToolCall> {
    let mut tool_calls: HashMap<usize, ToolCallAccum> = HashMap::new();

    for (index, id, name, arguments) in deltas {
        let entry = tool_calls.entry(*index).or_default();
        entry.delta_count += 1;

        if let Some(id) = id {
            entry.id_ever_seen = true;
            if !id.is_empty() {
                entry.id = id.clone();
            }
        }
        if let Some(name) = name
            && !name.is_empty()
        {
            entry.name = name.clone();
        }
        if let Some(args) = arguments {
            entry.arguments.push_str(args);
        }
    }

    // Observation only: this diagnostic does not alter what is returned below.
    // See DIAGNOSIS.md § FALSIFIED — this records whether assembly is the
    // origin of an empty tool_call_id reaching the provider, without masking
    // it with a synthesized id.
    for (index, accum) in &tool_calls {
        if accum.id.is_empty() {
            let index = *index;
            let delta_count = accum.delta_count;
            let id_ever_seen = accum.id_ever_seen;
            let function_name = accum.name.as_str();
            warn!(
                index,
                delta_count,
                id_ever_seen,
                function_name,
                "assemble_tool_calls_from_stream: tool call at index {index} has an empty id \
                 after {delta_count} delta(s) seen (id_ever_seen={id_ever_seen}, \
                 function_name={function_name:?})"
            );
        }
    }

    let mut calls: Vec<(usize, ToolCall)> = tool_calls
        .into_iter()
        .map(|(idx, accum)| {
            (
                idx,
                ToolCall {
                    id: accum.id,
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: accum.name,
                        arguments: accum.arguments,
                    },
                },
            )
        })
        .collect();

    calls.sort_by_key(|(idx, _)| *idx);
    calls.into_iter().map(|(_, tc)| tc).collect()
}

#[cfg(test)]
mod assemble_tool_calls_from_stream_tests {
    use super::*;

    /// The LIVE OpenRouter frame shape captured in DIAGNOSIS.md for the
    /// single-tool-call case: id+name arrive in frame 1 (with an empty
    /// `arguments` string), then three pure-argument frames with no id/name.
    /// Assembly is EXONERATED for this shape — the id survives intact and no
    /// warning fires. This pins the known-good path so a future change
    /// cannot silently regress it into the failure this task instruments.
    #[test]
    #[tracing_test::traced_test]
    fn exonerated_path_single_tool_call_live_frame_shape_keeps_id_and_does_not_warn() {
        let deltas: Vec<ToolCallDelta> = vec![
            (
                0,
                Some("toolu_01JZ5qVfPz5Jieu8HRSyAA41".to_string()),
                Some("get_weather".to_string()),
                Some(String::new()),
            ),
            (0, None, None, Some(String::new())),
            (0, None, None, Some("{\"city\": \"Paris".to_string())),
            (0, None, None, Some("\"}".to_string())),
        ];

        let calls = assemble_tool_calls_from_stream(&deltas);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "toolu_01JZ5qVfPz5Jieu8HRSyAA41");
        assert_eq!(calls[0].function.name, "get_weather");
        assert_eq!(calls[0].function.arguments, "{\"city\": \"Paris\"}");
        assert!(
            !logs_contain("has an empty id"),
            "assembly must not warn on the known-good, exonerated frame shape"
        );
    }

    /// No delta for index 0 ever carries an `id` field (not even `Some("")`).
    /// The warning must fire and must name index 0.
    #[test]
    #[tracing_test::traced_test]
    fn no_id_ever_supplied_for_an_index_warns_and_names_it() {
        let deltas: Vec<ToolCallDelta> = vec![
            (
                0,
                None,
                Some("get_weather".to_string()),
                Some(String::new()),
            ),
            (0, None, None, Some("{}".to_string())),
        ];

        let calls = assemble_tool_calls_from_stream(&deltas);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "", "id must still be empty — no synthesis");
        assert!(
            logs_contain("tool call at index 0 has an empty id"),
            "warning must fire and name index 0"
        );
        assert!(
            logs_contain("id_ever_seen=false"),
            "warning must record that no id field was ever seen for this index"
        );
        assert!(
            logs_contain("after 2 delta(s) seen"),
            "warning must record the delta count observed for this index"
        );
    }

    /// Parallel tool calls: index 0 gets an id, index 1 never does. This is
    /// the leading untested hypothesis for the production failure (DIAGNOSIS.md
    /// § FALSIFIED #3). The warning must fire for index 1 ONLY, and index 0's
    /// id must remain intact and unwarned.
    #[test]
    #[tracing_test::traced_test]
    fn parallel_tool_calls_warns_only_for_the_index_missing_an_id() {
        let deltas: Vec<ToolCallDelta> = vec![
            (
                0,
                Some("call_has_id_0".to_string()),
                Some("fn_a".to_string()),
                Some(String::new()),
            ),
            (1, None, Some("fn_b".to_string()), Some(String::new())),
            (0, None, None, Some("{\"a\":1}".to_string())),
            (1, None, None, Some("{\"b\":2}".to_string())),
        ];

        let calls = assemble_tool_calls_from_stream(&deltas);

        assert_eq!(calls.len(), 2);
        let call_0 = calls
            .iter()
            .find(|c| c.function.name == "fn_a")
            .expect("index 0 call");
        let call_1 = calls
            .iter()
            .find(|c| c.function.name == "fn_b")
            .expect("index 1 call");
        assert_eq!(
            call_0.id, "call_has_id_0",
            "index 0's id must remain intact"
        );
        assert_eq!(
            call_1.id, "",
            "index 1's id must still be empty — no synthesis"
        );
        assert!(
            logs_contain("tool call at index 1 has an empty id"),
            "warning must fire for index 1"
        );
        assert!(
            !logs_contain("tool call at index 0 has an empty id"),
            "warning must NOT fire for index 0 — its id is intact"
        );
    }
}

use anyhow::{Context, Result};
use futures::StreamExt;
use ironhermes_core::{
    ChatMessage, ChatRequest, ChatResponse, ChatStreamChunk, FunctionCall, ToolCall, ToolSchema,
    Usage,
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
    pub fn set_prompt_caching(
        &mut self,
        cfg: ironhermes_core::config::PromptCachingConfig,
    ) {
        self.prompt_caching = cfg;
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
        let url = format!("{}/chat/completions", self.base_url);

        // Phase 36.2 CR-09: when the (provider, model) pair is OpenRouter
        // Claude AND prompt_caching is enabled, route through the OpenRouter
        // Claude request builder so `cache_control` markers attach per the
        // system_and_3 strategy. Pre-fix this builder existed and was tested
        // but no production code path invoked it — the OpenRouter Claude
        // cache hits Plan 11 was meant to deliver never fired.
        let response = if crate::any_client::is_openrouter_claude(
            &self.provider_name,
            &resolved_model,
        ) && self.prompt_caching.enabled
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
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                                if val.get("error").is_some() {
                                    let msg = sse_error_to_bail_string(data, &val);
                                    let _ = tx.send(StreamEvent::ProviderError(msg)).await;
                                    return;
                                }
                            }
                            // D-05 (phase 36.14): not a recognised error envelope — preserve existing debug log.
                            debug!("Failed to parse stream chunk: {} — data: {}", _parse_err, data);
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

/// Assemble a complete ChatMessage from streaming tool call deltas.
pub fn assemble_tool_calls_from_stream(deltas: &[ToolCallDelta]) -> Vec<ToolCall> {
    let mut tool_calls: HashMap<usize, (String, String, String)> = HashMap::new();

    for (index, id, name, arguments) in deltas {
        let entry = tool_calls
            .entry(*index)
            .or_insert_with(|| (String::new(), String::new(), String::new()));

        if let Some(id) = id
            && !id.is_empty()
        {
            entry.0 = id.clone();
        }
        if let Some(name) = name
            && !name.is_empty()
        {
            entry.1 = name.clone();
        }
        if let Some(args) = arguments {
            entry.2.push_str(args);
        }
    }

    let mut calls: Vec<(usize, ToolCall)> = tool_calls
        .into_iter()
        .map(|(idx, (id, name, args))| {
            (
                idx,
                ToolCall {
                    id,
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name,
                        arguments: args,
                    },
                },
            )
        })
        .collect();

    calls.sort_by_key(|(idx, _)| *idx);
    calls.into_iter().map(|(_, tc)| tc).collect()
}

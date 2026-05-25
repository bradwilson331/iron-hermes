use anyhow::{Result, anyhow};
use async_trait::async_trait;
use ironhermes_core::config::PromptCachingConfig;
use ironhermes_core::types::{ContentPart, ImageUrl, MessageContent, Role};
use ironhermes_core::{
    ApiMode, ChatMessage, ChatResponse, ProviderResolver, ResolvedEndpoint, ToolSchema,
};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::agent_loop::AgentLoop;
use crate::anthropic_client::AnthropicClient;
use crate::client::{LlmClient, StreamEvent};

// =============================================================================
// AnyClient enum dispatch (D-07, D-08, D-10)
// =============================================================================

/// Universal client that dispatches to the correct backend based on ApiMode.
///
/// AnyClient is the type used by AgentLoop — it wraps either an OpenAI-compatible
/// LlmClient (ChatCompletions mode) or the native AnthropicClient (AnthropicMessages mode).
/// CodexResponses is not yet implemented; constructing it returns an error.
#[derive(Clone)]
pub enum AnyClient {
    ChatCompletions(LlmClient),
    AnthropicMessages(AnthropicClient),
}

impl std::fmt::Debug for AnyClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChatCompletions(_) => write!(f, "AnyClient::ChatCompletions(LlmClient)"),
            Self::AnthropicMessages(c) => write!(f, "AnyClient::AnthropicMessages({:?})", c),
        }
    }
}

impl AnyClient {
    /// Construct from a resolved provider endpoint.
    ///
    /// - `ApiMode::ChatCompletions` → wraps LlmClient
    /// - `ApiMode::AnthropicMessages` → wraps AnthropicClient
    /// - `ApiMode::CodexResponses` → returns error (not yet implemented)
    pub fn from_endpoint(endpoint: &ResolvedEndpoint) -> Result<Self> {
        match endpoint.api_mode {
            ApiMode::ChatCompletions => Ok(AnyClient::ChatCompletions(LlmClient::new(
                &endpoint.base_url,
                endpoint.api_key.as_deref().unwrap_or(""),
                &endpoint.default_model,
            ))),
            ApiMode::AnthropicMessages => Ok(AnyClient::AnthropicMessages(AnthropicClient::new(
                &endpoint.base_url,
                endpoint.api_key.as_deref().unwrap_or(""),
                &endpoint.default_model,
            ))),
            ApiMode::CodexResponses => Err(anyhow!(
                "Codex Responses API mode is not yet implemented. Use chat_completions or anthropic_messages."
            )),
        }
    }

    /// Construct from a resolved endpoint, but override the model.
    pub fn from_endpoint_with_model(endpoint: &ResolvedEndpoint, model: &str) -> Result<Self> {
        match endpoint.api_mode {
            ApiMode::ChatCompletions => Ok(AnyClient::ChatCompletions(LlmClient::new(
                &endpoint.base_url,
                endpoint.api_key.as_deref().unwrap_or(""),
                model,
            ))),
            ApiMode::AnthropicMessages => Ok(AnyClient::AnthropicMessages(AnthropicClient::new(
                &endpoint.base_url,
                endpoint.api_key.as_deref().unwrap_or(""),
                model,
            ))),
            ApiMode::CodexResponses => Err(anyhow!(
                "Codex Responses API mode is not yet implemented. Use chat_completions or anthropic_messages."
            )),
        }
    }

    /// Non-streaming chat completion — delegates to the inner client.
    pub async fn chat_completion(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSchema]>,
        model: Option<&str>,
        max_tokens: Option<usize>,
        temperature: Option<f64>,
        extra: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<ChatResponse> {
        match self {
            Self::ChatCompletions(c) => {
                c.chat_completion(messages, tools, model, max_tokens, temperature, extra)
                    .await
            }
            Self::AnthropicMessages(c) => {
                c.chat_completion(messages, tools, model, max_tokens, temperature, extra)
                    .await
            }
        }
    }

    /// Get the model name from the inner client.
    pub fn model(&self) -> &str {
        match self {
            Self::ChatCompletions(c) => c.model(),
            Self::AnthropicMessages(c) => c.model(),
        }
    }

    /// Streaming chat completion — delegates to the inner client.
    pub async fn chat_completion_stream(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSchema]>,
        model: Option<&str>,
        max_tokens: Option<usize>,
        temperature: Option<f64>,
        extra: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        match self {
            Self::ChatCompletions(c) => {
                c.chat_completion_stream(messages, tools, model, max_tokens, temperature, extra)
                    .await
            }
            Self::AnthropicMessages(c) => {
                c.chat_completion_stream(messages, tools, model, max_tokens, temperature, extra)
                    .await
            }
        }
    }
}

// =============================================================================
// Factory functions (D-03)
// =============================================================================

/// Build an AnyClient from a ProviderResolver for a given provider and model.
///
/// This is the single entry point for client construction (per D-03).
pub fn build_client(resolver: &ProviderResolver, provider: &str, model: &str) -> Result<AnyClient> {
    let endpoint = resolver
        .resolve(provider)
        .ok_or_else(|| anyhow!("Unknown provider: {provider}"))?;
    AnyClient::from_endpoint_with_model(endpoint, model)
}

/// Build an AnyClient for the main configured provider.
pub fn build_main_client(resolver: &ProviderResolver) -> Result<AnyClient> {
    let endpoint = resolver.resolve_for_main();
    AnyClient::from_endpoint(endpoint)
}

/// Build an AnyClient for an auxiliary role (vision, compression, etc).
///
/// Returns `None` if the role is not configured.
pub fn build_role_client(resolver: &ProviderResolver, role: &str) -> Result<Option<AnyClient>> {
    match resolver.resolve_role(role) {
        Some(endpoint) => Ok(Some(AnyClient::from_endpoint(&endpoint)?)),
        None => Ok(None),
    }
}

/// Wire a fallback provider client onto an AgentLoop, if one is configured.
///
/// Reads `resolver.resolve_for_main().fallback_providers.first()` and, if present,
/// attempts to build a client for that provider. On success chains `.with_fallback()`;
/// on any failure (provider name not found OR client build error) emits a
/// `tracing::warn!` and returns the agent unchanged.
///
/// Used at every AgentLoop construction site to close PROV-07 gaps.
pub fn wire_fallback_if_configured(
    mut agent: AgentLoop,
    resolver: &ProviderResolver,
) -> AgentLoop {
    let main_endpoint = resolver.resolve_for_main();
    if let Some(fb_name) = main_endpoint.fallback_providers.first() {
        if let Some(fb_endpoint) = resolver.resolve(fb_name) {
            match build_client(resolver, fb_name, &fb_endpoint.default_model) {
                Ok(fb_client) => {
                    agent = agent.with_fallback(fb_client);
                }
                Err(e) => {
                    tracing::warn!(
                        error = ?e,
                        "fallback provider '{}' client build failed — running without fallback",
                        fb_name
                    );
                }
            }
        } else {
            tracing::warn!(
                "fallback provider '{}' not found in resolver — running without fallback",
                fb_name
            );
        }
    }
    agent
}

// =============================================================================
// AnyClientVisionHandle — VisionClientHandle impl for production use (OQ-5 closure)
// =============================================================================

/// Production implementation of `VisionClientHandle` for `browser_vision`.
///
/// Wraps an `Arc<ProviderResolver>` and uses `build_role_client("vision")` to
/// follow the Phase 26 D-07 cascade:
///   1. `model.roles["vision"]` per-task override
///   2. `auxiliary` block fallback
///   3. Fall through to main provider (if supports_vision)
///
/// Constructed in `ironhermes-cli/src/main.rs` and passed to
/// `register_browser_tools_with_vision` so that `BrowserVisionTool` routes
/// multimodal calls through the correct endpoint.
pub struct AnyClientVisionHandle {
    resolver: Arc<ProviderResolver>,
}

impl AnyClientVisionHandle {
    pub fn new(resolver: Arc<ProviderResolver>) -> Self {
        Self { resolver }
    }
}

#[async_trait]
impl ironhermes_tools::browser_vision::VisionClientHandle for AnyClientVisionHandle {
    async fn vision_call(&self, prompt: String, image_data_url: String) -> anyhow::Result<String> {
        // D-07 cascade: vision role → auxiliary → main provider.
        let client = match build_role_client(&self.resolver, "vision")? {
            Some(c) => c,
            None => build_main_client(&self.resolver)?,
        };

        // Build a multimodal message: text prompt + image data URL (D-08 PNG base64).
        let messages = vec![ChatMessage {
            role: Role::User,
            content: Some(MessageContent::Parts(vec![
                ContentPart::Text { text: prompt },
                ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: image_data_url,
                        detail: Some("auto".to_string()),
                    },
                },
            ])),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        }];

        let response = client
            .chat_completion(&messages, None, None, Some(1024), None, None)
            .await?;

        // Extract text content from choices[0].message.content
        let text = response
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .and_then(|mc| match mc {
                ironhermes_core::types::MessageContent::Text(s) => Some(s),
                ironhermes_core::types::MessageContent::Parts(parts) => {
                    parts.into_iter().find_map(|p| {
                        if let ironhermes_core::types::ContentPart::Text { text } = p {
                            Some(text)
                        } else {
                            None
                        }
                    })
                }
            })
            .unwrap_or_else(|| "(no vision response)".to_string());
        Ok(text)
    }
}

// =============================================================================
// AnyClientSummarizationHandle — SummarizationClientHandle impl (Phase 25.2 D-13)
// =============================================================================

/// Production implementation of `SummarizationClientHandle` for `web_extract`
/// (Phase 25.2 D-13 — second consumer of `resolve_role` after Phase 25.1's vision).
///
/// Wraps an `Arc<ProviderResolver>` and uses `build_role_client("summarization")` to
/// follow the Phase 26 D-07 cascade:
///   1. `model.roles["summarization"]` per-task override
///   2. `auxiliary` block fallback (`auxiliary.summary` → general aux)
///   3. Fall through to main provider (always succeeds — no None reaches WebExtractTool)
///
/// Constructed in `ironhermes-cli/src/main.rs` (run_chat / run_single / run_gateway)
/// and passed to `register_web_extract_tool` so that `WebExtractTool` routes
/// summarization calls through the correct endpoint per the operator's config.yaml.
pub struct AnyClientSummarizationHandle {
    resolver: Arc<ProviderResolver>,
}

impl AnyClientSummarizationHandle {
    pub fn new(resolver: Arc<ProviderResolver>) -> Self {
        Self { resolver }
    }
}

#[async_trait]
impl ironhermes_core::SummarizationClientHandle for AnyClientSummarizationHandle {
    async fn summarize_call(
        &self,
        system_prompt: String,
        user_prompt: String,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        // Phase 26 D-07 cascade: summarization role → auxiliary → main provider.
        let client = match build_role_client(&self.resolver, "summarization")? {
            Some(c) => c,
            None => build_main_client(&self.resolver)?,
        };

        // Text-only message vector (simpler than vision multimodal payload).
        let messages = vec![
            ChatMessage {
                role: Role::System,
                content: Some(MessageContent::Text(system_prompt)),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                is_recall_context: false,
            },
            ChatMessage {
                role: Role::User,
                content: Some(MessageContent::Text(user_prompt)),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                is_recall_context: false,
            },
        ];

        let response = client
            .chat_completion(&messages, None, None, Some(max_tokens as usize), None, None)
            .await?;

        // Extract text content from choices[0].message.content (same shape as vision handle).
        let text = response
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .and_then(|mc| match mc {
                ironhermes_core::types::MessageContent::Text(s) => Some(s),
                ironhermes_core::types::MessageContent::Parts(parts) => {
                    parts.into_iter().find_map(|p| {
                        if let ironhermes_core::types::ContentPart::Text { text } = p {
                            Some(text)
                        } else {
                            None
                        }
                    })
                }
            })
            .unwrap_or_else(|| "(no summarization response)".to_string());
        Ok(text)
    }
}

// =============================================================================
// Phase 36.2 Plan 11 (OPTIONAL STRETCH per D-CACHE-01):
// OpenRouter Claude cache_control attachment on the ChatCompletions arm.
// =============================================================================
//
// Wave 1/2 Plan 05 invariant: `grep -c 'cache_control' any_client.rs == 0`.
// Wave 3 (THIS plan): relaxed to `>= 1`, gated by `is_openrouter_claude`.
//
// Non-Claude OpenRouter models and non-OpenRouter providers continue to
// receive ZERO markers from the ChatCompletions arm — preserves D-CACHE-01
// cut-line semantics so the Anthropic-side PARITY from Plan 05 is intact.
//
// Assumption A1 (verified against OpenRouter docs 2026-05-25 §"Anthropic
// Claude" §"TTL Options"): OpenRouter accepts the SAME `cache_control`
// envelope as native Anthropic: `{ "type": "ephemeral" }` or
// `{ "type": "ephemeral", "ttl": "1h" }`. Source:
//   https://openrouter.ai/docs/guides/best-practices/prompt-caching.mdx

/// Phase 36.2 Plan 11: provider+model gating predicate for OpenRouter Claude
/// routing on the ChatCompletions arm.
///
/// Returns `true` ONLY when `provider` is exactly `"openrouter"` AND `model`
/// is in the Claude family (prefixed `"anthropic/claude"` OR bare `"claude-"`
/// — OpenRouter sometimes routes without the provider prefix).
///
/// All other (provider, model) pairs return `false`. Non-OpenRouter providers
/// (OpenAI, Nous, custom endpoints) and non-Claude OpenRouter models continue
/// to receive ZERO `cache_control` markers from the ChatCompletions arm — the
/// AnthropicMessages arm (Plan 05) handles native Anthropic separately.
pub fn is_openrouter_claude(provider: &str, model: &str) -> bool {
    provider == "openrouter"
        && (model.starts_with("anthropic/claude") || model.starts_with("claude-"))
}

/// Phase 36.2 Plan 11: OpenRouter Claude `cache_control` envelope.
///
/// Serializes as `{"type":"ephemeral","ttl":"5m"|"1h"}` — identical to native
/// Anthropic per Assumption A1. Attached to OpenAI-compat content blocks
/// (NOT messages themselves) to mark cache breakpoints. Per the
/// `system_and_3` strategy, exactly 4 markers max per request: 1 system + last
/// 3 non-system messages.
#[derive(Debug, Clone, Serialize)]
pub struct OpenRouterCacheControl {
    #[serde(rename = "type")]
    ty: String,
    ttl: String,
}

impl OpenRouterCacheControl {
    fn ephemeral(ttl: &'static str) -> Self {
        Self {
            ty: "ephemeral".to_string(),
            ttl: ttl.to_string(),
        }
    }
}

/// Phase 36.2 Plan 11: OpenAI-compat content block with optional
/// `cache_control` for OpenRouter Claude routing.
///
/// Maps to the OpenAI Chat Completions content-block shape:
/// `{ "type": "text", "text": "..." }` with an optional `cache_control`
/// field — accepted by OpenRouter when routed to Claude.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OpenRouterContentBlock {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<OpenRouterCacheControl>,
    },
}

/// Phase 36.2 Plan 11: OpenAI-compat message used on the OpenRouter Claude
/// path. Identical wire-shape to a normal Chat Completions message except
/// `content` is always an array (required for `cache_control` to attach to a
/// block).
#[derive(Debug, Clone, Serialize)]
pub struct OpenRouterMessage {
    pub role: String,
    pub content: Vec<OpenRouterContentBlock>,
}

/// Phase 36.2 Plan 11: OpenAI-compat ChatCompletions request body used for
/// OpenRouter Claude routing when `prompt_caching.enabled` is true.
///
/// Serializes to the same JSON shape OpenRouter's Chat Completions endpoint
/// accepts, with `cache_control` markers attached per system_and_3 strategy.
/// Non-Claude OpenRouter models and non-OpenRouter providers MUST NOT use
/// this builder (use the regular `LlmClient::chat_completion` path instead).
#[derive(Debug, Clone, Serialize)]
pub struct OpenRouterChatRequest {
    pub model: String,
    pub messages: Vec<OpenRouterMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

/// Phase 36.2 Plan 11: build an [`OpenRouterChatRequest`] with optional
/// `cache_control` markers attached per the `system_and_3` strategy.
///
/// **Gating** (Plan 11 D-CACHE-01 cut-line invariant):
/// - Markers attach ONLY when ALL three conditions hold:
///   1. `is_openrouter_claude(provider, model)` returns true
///   2. `prompt_caching.enabled` is true
///   3. The conversation has at least one message
/// - Otherwise the request body is byte-identical to a plain OpenAI-compat
///   ChatCompletions request with zero `cache_control` fields anywhere.
///
/// **Attachment strategy** (`system_and_3`, mirrors Plan 05 anthropic_client):
/// - 1 marker on the system block (if present) — only the LAST system message
///   gets the marker if multiple are present.
/// - 1 marker each on the LAST `min(3, n)` non-system messages, attached to
///   each message's last content block (`saturating_sub(3)..n`).
/// - 4 markers max per request — matches Anthropic's documented breakpoint
///   cap (which OpenRouter inherits per Assumption A1).
pub fn build_openrouter_chat_request(
    provider: &str,
    model: &str,
    messages: &[ChatMessage],
    max_tokens: Option<usize>,
    temperature: Option<f64>,
    stream: Option<bool>,
    prompt_caching: &PromptCachingConfig,
) -> OpenRouterChatRequest {
    // Adapt every ChatMessage to OpenRouter's content-block array shape.
    // Non-text content parts (image_url, etc.) are not common on cached
    // prefixes and are passed through as text where possible; the cached
    // layers must be stable per D-CACHE-04, so dynamic image data should
    // not appear in the cached prefix anyway.
    let mut adapted: Vec<OpenRouterMessage> = messages
        .iter()
        .map(adapt_chat_message_to_openrouter)
        .collect();

    let attach = prompt_caching.enabled && is_openrouter_claude(provider, model);

    if attach {
        let ttl = prompt_caching.ttl.as_anthropic_ttl();
        attach_cache_control_to_chat_completions(&mut adapted, ttl);
    }

    OpenRouterChatRequest {
        model: model.to_string(),
        messages: adapted,
        max_tokens,
        temperature,
        stream,
    }
}

/// Adapt an OpenAI-compat `ChatMessage` to an OpenRouter `OpenRouterMessage`
/// whose `content` is always a `Vec<OpenRouterContentBlock>` (array form,
/// required so `cache_control` can attach to a block per Anthropic's API
/// which OpenRouter passes through).
fn adapt_chat_message_to_openrouter(msg: &ChatMessage) -> OpenRouterMessage {
    let role = match msg.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
    .to_string();

    let blocks: Vec<OpenRouterContentBlock> = match &msg.content {
        Some(MessageContent::Text(s)) => vec![OpenRouterContentBlock::Text {
            text: s.clone(),
            cache_control: None,
        }],
        Some(MessageContent::Parts(parts)) => parts
            .iter()
            .map(|p| match p {
                ContentPart::Text { text } => OpenRouterContentBlock::Text {
                    text: text.clone(),
                    cache_control: None,
                },
                // Phase 36.2 Plan 11: image_url parts are passed through as
                // their text URL on this path (cached-layer-stability D-CACHE-04
                // forbids dynamic image data in cached prefix; non-cached
                // tails use the normal LlmClient path, not this builder).
                ContentPart::ImageUrl { image_url } => OpenRouterContentBlock::Text {
                    text: format!("[image: {}]", image_url.url),
                    cache_control: None,
                },
            })
            .collect(),
        None => Vec::new(),
    };

    OpenRouterMessage {
        role,
        content: blocks,
    }
}

/// Phase 36.2 Plan 11: attach `cache_control` markers using the `system_and_3`
/// strategy on an OpenAI-compat (OpenRouter Claude) message list.
///
/// **Marker count rules** (max 4 per request — Anthropic API constraint
/// passed through OpenRouter):
/// - 1 marker on the LAST system message's last content block (if any
///   system message exists).
/// - 1 marker each on the LAST `min(3, non_system_count)` non-system
///   messages, attached to each message's last content block.
///
/// Mirrors `anthropic_client::attach_cache_control_markers` semantics but
/// operates on the OpenAI-compat content-block shape.
fn attach_cache_control_to_chat_completions(messages: &mut [OpenRouterMessage], ttl: &'static str) {
    // Mark the LAST system message's last content block.
    if let Some(last_system) = messages.iter_mut().rev().find(|m| m.role == "system")
        && let Some(last_block) = last_system.content.last_mut()
    {
        match last_block {
            OpenRouterContentBlock::Text { cache_control, .. } => {
                *cache_control = Some(OpenRouterCacheControl::ephemeral(ttl));
            }
        }
    }

    // Collect indices of non-system messages, then mark the last min(3, n).
    let non_system_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role != "system")
        .map(|(i, _)| i)
        .collect();
    let n = non_system_indices.len();
    let start = n.saturating_sub(3);
    for &idx in &non_system_indices[start..n] {
        if let Some(last_block) = messages[idx].content.last_mut() {
            match last_block {
                OpenRouterContentBlock::Text { cache_control, .. } => {
                    *cache_control = Some(OpenRouterCacheControl::ephemeral(ttl));
                }
            }
        }
    }
}

/// Phase 36.2 Plan 11 test helper: thin re-export of
/// [`build_openrouter_chat_request`] for the integration tests in
/// `tests/openrouter_cache_control.rs`.
///
/// Identical gating semantics to the production builder so tests exercise
/// the exact code path. Mirrors the pattern from
/// `anthropic_client::build_anthropic_request_for_test` (Plan 05).
#[doc(hidden)]
pub fn build_openrouter_chat_request_for_test(
    provider: &str,
    model: &str,
    messages: &[ChatMessage],
    prompt_caching: &PromptCachingConfig,
) -> OpenRouterChatRequest {
    build_openrouter_chat_request(
        provider,
        model,
        messages,
        Some(4096),
        None,
        Some(false),
        prompt_caching,
    )
}

/// Phase 36.2 Plan 11 test helper: serialize an [`OpenRouterChatRequest`] to
/// JSON for marker-count substring assertions in integration tests.
///
/// Mirrors `anthropic_client::serialize_request_for_test` (Plan 05).
#[doc(hidden)]
pub fn serialize_openrouter_request_for_test(req: &OpenRouterChatRequest) -> Result<String> {
    serde_json::to_string(req).map_err(|e| anyhow!("serialize OpenRouterChatRequest: {e}"))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::{ApiMode, Config, ResolvedEndpoint};

    fn make_endpoint(api_mode: ApiMode) -> ResolvedEndpoint {
        ResolvedEndpoint {
            base_url: "https://api.example.com".to_string(),
            api_key: Some("test-key".to_string()),
            api_mode,
            default_model: "test-model".to_string(),
            fallback_providers: vec![],
            model_metadata: None,
            config_context_length: None,
        }
    }

    // Test: AnyClient::ChatCompletions wraps LlmClient
    #[test]
    fn test_any_client_chat_completions_variant() {
        let endpoint = make_endpoint(ApiMode::ChatCompletions);
        let client = AnyClient::from_endpoint(&endpoint).unwrap();
        assert!(matches!(client, AnyClient::ChatCompletions(_)));
    }

    // Test: AnyClient::AnthropicMessages wraps AnthropicClient
    #[test]
    fn test_any_client_anthropic_messages_variant() {
        let endpoint = make_endpoint(ApiMode::AnthropicMessages);
        let client = AnyClient::from_endpoint(&endpoint).unwrap();
        assert!(matches!(client, AnyClient::AnthropicMessages(_)));
    }

    // Test: AnyClient::from_endpoint with ApiMode::ChatCompletions creates ChatCompletions variant
    #[test]
    fn test_from_endpoint_chat_completions() {
        let endpoint = make_endpoint(ApiMode::ChatCompletions);
        let result = AnyClient::from_endpoint(&endpoint);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), AnyClient::ChatCompletions(_)));
    }

    // Test: AnyClient::from_endpoint with ApiMode::AnthropicMessages creates AnthropicMessages variant
    #[test]
    fn test_from_endpoint_anthropic_messages() {
        let endpoint = make_endpoint(ApiMode::AnthropicMessages);
        let result = AnyClient::from_endpoint(&endpoint);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), AnyClient::AnthropicMessages(_)));
    }

    // Test: AnyClient::from_endpoint with ApiMode::CodexResponses returns error
    #[test]
    fn test_from_endpoint_codex_responses_errors() {
        let endpoint = make_endpoint(ApiMode::CodexResponses);
        let result = AnyClient::from_endpoint(&endpoint);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not yet implemented"),
            "Error should contain 'not yet implemented': {err_msg}"
        );
    }

    // Test: ProviderResolver::build_client returns AnyClient for valid provider
    #[test]
    fn test_build_client_valid_provider() {
        let config = Config::default();
        let resolver = ProviderResolver::build(&config).unwrap();
        let result = build_client(&resolver, "openrouter", "gpt-4");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), AnyClient::ChatCompletions(_)));
    }

    // Test: ProviderResolver::build_client returns error for unknown provider
    #[test]
    fn test_build_client_unknown_provider() {
        let config = Config::default();
        let resolver = ProviderResolver::build(&config).unwrap();
        let result = build_client(&resolver, "nonexistent-provider", "gpt-4");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Unknown provider"),
            "Error should mention unknown provider: {err_msg}"
        );
    }

    // Test: build_main_client returns AnyClient for main provider
    #[test]
    fn test_build_main_client() {
        let config = Config::default();
        let resolver = ProviderResolver::build(&config).unwrap();
        let result = build_main_client(&resolver);
        assert!(result.is_ok());
    }

    // Test: build_role_client returns None for unconfigured role
    #[test]
    fn test_build_role_client_missing_role() {
        let config = Config::default();
        let resolver = ProviderResolver::build(&config).unwrap();
        let result = build_role_client(&resolver, "nonexistent_role").unwrap();
        assert!(result.is_none());
    }

    // Test: AnyClient has chat_completion and chat_completion_stream that delegate to inner client
    // (structural test — verifies method signatures compile and dispatch)
    #[test]
    fn test_any_client_has_delegation_methods() {
        // Just verify both variants compile and have the methods available
        let endpoint_cc = make_endpoint(ApiMode::ChatCompletions);
        let endpoint_am = make_endpoint(ApiMode::AnthropicMessages);

        let cc = AnyClient::from_endpoint(&endpoint_cc).unwrap();
        let am = AnyClient::from_endpoint(&endpoint_am).unwrap();

        // Verify debug output (structural check)
        let cc_debug = format!("{:?}", cc);
        let am_debug = format!("{:?}", am);
        assert!(cc_debug.contains("ChatCompletions"));
        assert!(am_debug.contains("AnthropicMessages"));
    }

    /// Phase 25.2 D-13: dyn-compatibility test — AnyClientSummarizationHandle
    /// can be coerced into Arc<dyn SummarizationClientHandle>, which is the form
    /// register_web_extract_tool consumes.
    #[test]
    fn test_any_client_summarization_handle_constructible() {
        let config = Config::default();
        let resolver = ironhermes_core::ProviderResolver::build(&config).unwrap();
        let handle = AnyClientSummarizationHandle::new(Arc::new(resolver));
        let _: Arc<dyn ironhermes_core::SummarizationClientHandle> = Arc::new(handle);
    }

    /// Phase 25.2 D-20 + D-27: end-to-end smoke test that the production wireup path
    /// (AnyClientSummarizationHandle + register_web_extract_tool) produces a registry
    /// where `web_extract` appears in get_definitions() output.
    ///
    /// This validates the full Task 1 + Task 2 wiring at unit-test time without needing
    /// a live AgentLoop — exactly what the production binary does in run_chat /
    /// run_single / run_gateway after Plan 14 ships.
    #[tokio::test]
    async fn web_extract_tool_appears_in_definitions_after_wireup() {
        use ironhermes_core::SkillRegistry;
        use ironhermes_tools::ToolRegistry;

        // 1. Build resolver from default config.
        let config = Config::default();
        let resolver = ironhermes_core::ProviderResolver::build(&config).unwrap();

        // 2. Construct the cascade-aware summarization handle and coerce to dyn trait.
        let handle: Arc<dyn ironhermes_core::SummarizationClientHandle> =
            Arc::new(AnyClientSummarizationHandle::new(Arc::new(resolver)));

        // 3. Construct an empty SkillRegistry from a tempdir (mirrors the production load).
        let tmp = tempfile::tempdir().expect("tempdir");
        let skill_registry = Arc::new(SkillRegistry::load(tmp.path()));

        // 4. Wire the production registration path that Plan 14 Task 2 added to the CLI.
        let mut registry = ToolRegistry::new();
        registry.register_web_extract_tool(handle, skill_registry);

        // 5. Assert web_extract appears in get_definitions() — proves the tool is
        //    discoverable by AgentLoop (which calls get_definitions to build tool schemas).
        let defs = registry.get_definitions(None);
        // ironhermes_core::ToolSchema is OpenAI-compatible: { type, function: { name, ... } }
        let names: Vec<String> = defs.iter().map(|d| d.function.name.clone()).collect();
        assert!(
            names.iter().any(|n| n == "web_extract"),
            "D-20 + D-27: web_extract must appear in get_definitions() after register_web_extract_tool. Got: {:?}",
            names
        );
    }
}

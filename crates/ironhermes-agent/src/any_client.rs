use anyhow::{anyhow, Result};
use ironhermes_core::{ApiMode, ChatMessage, ChatResponse, ProviderResolver, ResolvedEndpoint, ToolSchema};
use std::collections::HashMap;
use tokio::sync::mpsc;

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
                c.chat_completion(messages, tools, model, max_tokens, temperature, extra).await
            }
            Self::AnthropicMessages(c) => {
                c.chat_completion(messages, tools, model, max_tokens, temperature, extra).await
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
                c.chat_completion_stream(messages, tools, model, max_tokens, temperature, extra).await
            }
            Self::AnthropicMessages(c) => {
                c.chat_completion_stream(messages, tools, model, max_tokens, temperature, extra).await
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
        assert!(err_msg.contains("not yet implemented"), "Error should contain 'not yet implemented': {err_msg}");
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
        assert!(err_msg.contains("Unknown provider"), "Error should mention unknown provider: {err_msg}");
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
}

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Async callback type for sampling: given a prompt and optional model hint,
/// return the LLM completion string.
///
/// This is the closure/function signature that callers must provide to enable
/// server-initiated LLM requests (D-03). The future must be `'static + Send`.
pub type SamplingCallback = Arc<
    dyn Fn(String, Option<String>) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send + 'static>>
        + Send
        + Sync,
>;

/// Rate-limited sampling handler for server-initiated LLM completion requests (D-03).
///
/// MCP servers can request LLM completions via the sampling/createMessage protocol.
/// This handler wraps the callback with rate limiting (max_rpm) to prevent amplification
/// attacks from malicious MCP servers (T-21.2-07).
///
/// The `calls_this_minute` counter is approximate — it is not reset on a per-minute
/// schedule in this implementation. The primary protection is the hard cap; callers
/// who need strict per-minute windowing should wrap this with a more sophisticated
/// rate limiter.
pub struct SamplingHandler {
    callback: SamplingCallback,
    /// Monotonically increasing call counter. Checked against max_rpm.
    calls_this_minute: AtomicU32,
    /// Maximum allowed calls before rate limiting kicks in.
    max_rpm: u32,
    /// Maximum tool rounds per sampling request (for future use).
    #[allow(dead_code)]
    max_tool_rounds: u32,
}

impl SamplingHandler {
    /// Create a new `SamplingHandler`.
    ///
    /// - `callback`: async function called with `(prompt, model_hint)` that returns LLM completion
    /// - `max_rpm`: maximum requests per minute (default from config: 10)
    /// - `max_tool_rounds`: maximum tool rounds per sampling session (default from config: 5)
    pub fn new(callback: SamplingCallback, max_rpm: u32, max_tool_rounds: u32) -> Self {
        Self {
            callback,
            calls_this_minute: AtomicU32::new(0),
            max_rpm,
            max_tool_rounds,
        }
    }

    /// Handle a sampling/createMessage request from an MCP server.
    ///
    /// Returns `Err` if the rate limit is exceeded (T-21.2-07 mitigation).
    pub async fn handle_sampling_request(
        &self,
        prompt: String,
        model_hint: Option<String>,
    ) -> anyhow::Result<String> {
        let count = self.calls_this_minute.fetch_add(1, Ordering::SeqCst);
        if count >= self.max_rpm {
            return Err(anyhow::anyhow!(
                "Sampling rate limit exceeded ({} rpm)",
                self.max_rpm
            ));
        }
        (self.callback)(prompt, model_hint).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sampling_handler_calls_callback() {
        let callback: SamplingCallback = Arc::new(|prompt, _model_hint| {
            Box::pin(async move { Ok(format!("response to: {}", prompt)) })
        });
        let handler = SamplingHandler::new(callback, 10, 5);
        let result = handler
            .handle_sampling_request("hello".to_string(), None)
            .await
            .unwrap();
        assert_eq!(result, "response to: hello");
    }

    #[tokio::test]
    async fn test_sampling_handler_rate_limit() {
        let callback: SamplingCallback =
            Arc::new(|_, _| Box::pin(async move { Ok("ok".to_string()) }));
        let handler = SamplingHandler::new(callback, 2, 5);

        // First two calls succeed
        handler
            .handle_sampling_request("a".to_string(), None)
            .await
            .unwrap();
        handler
            .handle_sampling_request("b".to_string(), None)
            .await
            .unwrap();

        // Third call exceeds limit
        let result = handler
            .handle_sampling_request("c".to_string(), None)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("rate limit"));
    }

    #[tokio::test]
    async fn test_sampling_handler_passes_model_hint() {
        let callback: SamplingCallback = Arc::new(|_prompt, model_hint| {
            Box::pin(async move {
                Ok(model_hint.unwrap_or_else(|| "no-hint".to_string()))
            })
        });
        let handler = SamplingHandler::new(callback, 10, 5);
        let result = handler
            .handle_sampling_request("test".to_string(), Some("claude-3".to_string()))
            .await
            .unwrap();
        assert_eq!(result, "claude-3");
    }
}

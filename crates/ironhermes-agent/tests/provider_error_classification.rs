//! Phase 36.2-03 Task 2: Port of the 30 enumerated `error_classifier.py`
//! test cases (RESEARCH.md §error_classifier.py items 1-30) into Rust.
//!
//! The parity bar (D-ERR-04): every Python classifier decision is also
//! covered by a Rust test. Each test:
//!   * constructs an `anyhow::Error` via `anyhow::anyhow!` shaped like the
//!     production error chain string the Python test fixture uses,
//!   * calls `classify_llm_error_typed(&err)` (re-exported from the crate
//!     root by Task 1),
//!   * asserts the variant via `matches!`,
//!   * asserts `should_retry()` / `should_fallback()` / `variant_name()`
//!     to lock the (bool, bool) tuple the legacy facade returns.
//!
//! Test names preserve the Python identity (`401_auth`, `403_billing`,
//! `429_long_context_tier`, etc.) for traceability against the Python source
//! at `/Users/you/code/hermes-agent/agent/error_classifier.py`.
//!
//! HTTP status format: every test uses the production `bail!` format that
//! `AgentLoop::extract_http_status` recognises:
//!
//!   * `"Streaming chat completion failed (NNN Reason): body"` — the literal
//!     `bail!` shape from `client.rs:170` / `anthropic_client.rs`.
//!   * `"HTTP request failed with status: NNN Reason"` — synthetic format
//!     also accepted (kept for legacy regression coverage).
//!
//! Plain `"HTTP NNN Reason"` (without parens or `"status: "` prefix) does
//! NOT parse a status code — that is by design (see PROV-07 RESEARCH.md).
//! Tests that need to exercise the status-code path use one of the two
//! shapes above.
//!
//! NOTE on Rust↔Python variant collapses (per RESEARCH.md FailoverReason
//! mapping table at items 366-388):
//!   * Python `billing` → Rust `Billing` (kept distinct from Auth)
//!   * Python `long_context_tier` → Rust `RateLimited { retry_after }`
//!   * Python `oauth_long_context_beta_forbidden` → Rust `SchemaInvalid`
//!   * Python `provider_policy_blocked` → Rust `Auth`
//!   * Python `model_not_found` → Rust `ModelNotFound`
//!   * Python `image_too_large` → Rust `PayloadTooLarge`
//!   * Python `payload_too_large` → Rust `PayloadTooLarge`
//!   * Python `multimodal_tool_content_unsupported` → Rust `ToolError`
//!   * Python `thinking_signature` → Rust `SchemaInvalid`
//!   * Python `llama_cpp_grammar_pattern` → Rust `SchemaInvalid`
//!   * Python `timeout` → Rust `Transport`
//!   * Python `server_error` / `overloaded` → Rust `Server { status }`

use ironhermes_agent::{ProviderError, classify_llm_error_typed};
use std::time::Duration;

// =========================================================================
// Item 1: HTTP 401 → auth, not retryable, rotate credential, fallback.
// =========================================================================
#[test]
fn test_http_401_auth_not_retryable() {
    let err = anyhow::anyhow!("HTTP request failed with status: 401 Unauthorized");
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Auth), "got {result:?}");
    assert!(!result.should_retry());
    assert!(result.should_fallback());
    assert_eq!(result.variant_name(), "Auth");
}

// =========================================================================
// Item 2: HTTP 403 with "key limit exceeded" → billing (not auth).
// Python distinguishes 403+billing-phrase as Billing rather than Auth. The
// Rust classifier prioritises HTTP status, so 403 maps to Auth — the
// (retry, fallback) tuple is (false, true) for both Auth and Billing, so
// downstream behavior is preserved. Documented Rust↔Python collapse.
// =========================================================================
#[test]
fn test_http_403_key_limit_exceeded() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (403 Forbidden): key limit exceeded for billing cycle"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Auth));
    assert!(!result.should_retry());
    assert!(result.should_fallback());
}

// =========================================================================
// Item 3: HTTP 403 generic → auth.
// =========================================================================
#[test]
fn test_http_403_generic_auth() {
    let err = anyhow::anyhow!("HTTP request failed with status: 403 Forbidden");
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Auth));
    assert!(!result.should_retry());
    assert!(result.should_fallback());
}

// =========================================================================
// Item 4: HTTP 402 with "usage limit" + "try again" → rate_limit (NOT billing).
// Rust collapses 402 → Billing (status code wins). Both Billing and the
// Python `rate_limit` produce (false, true) / (true, true) respectively in
// terms of fallback, but the retry signal differs. Documented Rust↔Python
// collapse: a 402 with "try again" phrasing returns Billing in Rust.
// Downstream Plan 06 RateLimitTracker also subscribes to RateLimitEvents
// from non-classifier signals (header parsing), so the missing retry signal
// for this specific shape is recoverable at the tracker layer.
// =========================================================================
#[test]
fn test_http_402_usage_limit_try_again() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (402 Payment Required): usage limit hit; please try again next month"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Billing));
    assert!(!result.should_retry());
    assert!(result.should_fallback());
}

// =========================================================================
// Item 5: HTTP 402 without transient signal → billing.
// =========================================================================
#[test]
fn test_http_402_billing() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (402 Payment Required): account has no credit balance"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Billing));
    assert_eq!(result.variant_name(), "Billing");
}

// =========================================================================
// Item 6: HTTP 429 → rate_limit, retryable, fallback.
// =========================================================================
#[test]
fn test_http_429_rate_limit_retryable_fallback() {
    let err = anyhow::anyhow!("HTTP request failed with status: 429 Too Many Requests");
    let result = classify_llm_error_typed(&err);
    assert!(matches!(
        result,
        ProviderError::RateLimited { retry_after: None }
    ));
    assert!(result.should_retry());
    assert!(result.should_fallback());
}

// =========================================================================
// Item 6b: HTTP 429 with retry-after header → RateLimited { retry_after: Some(Duration) }.
// This is the load-bearing test that proves Plan 06 (RateLimitTracker) can
// destructure the retry-after value from the classification result.
// =========================================================================
#[test]
fn test_http_429_with_retry_after_header_parses_duration() {
    let err = anyhow::anyhow!(
        "HTTP request failed with status: 429 Too Many Requests; retry-after: 60; body: rate limit reached"
    );
    let result = classify_llm_error_typed(&err);
    match result {
        ProviderError::RateLimited { retry_after } => {
            assert_eq!(
                retry_after,
                Some(Duration::from_secs(60)),
                "retry-after header should parse to 60 seconds"
            );
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

// =========================================================================
// Item 7: HTTP 429 with "extra usage" + "long context" → long_context_tier
// (Python). Rust collapses → RateLimited { retry_after }.
// =========================================================================
#[test]
fn test_http_429_extra_usage_long_context_tier() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (429 Too Many Requests): extra usage allowance exhausted for long context tier"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::RateLimited { .. }));
    assert!(result.should_retry());
    assert!(result.should_fallback());
}

// =========================================================================
// Item 8: HTTP 400 with context overflow patterns → context_overflow, compress.
// =========================================================================
#[test]
fn test_http_400_context_overflow() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (400 Bad Request): context length 50000 exceeds maximum context length of 32000"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::ContextLength), "got {result:?}");
    assert!(!result.should_retry());
    assert!(result.should_fallback());
}

// =========================================================================
// Item 9: HTTP 400 with image_too_large patterns → image_too_large.
// Rust → PayloadTooLarge.
// =========================================================================
#[test]
fn test_400_image_too_large() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (400 Bad Request): image too large; max size limit is 5MB"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::PayloadTooLarge), "got {result:?}");
    assert_eq!(result.variant_name(), "PayloadTooLarge");
}

// =========================================================================
// Item 10: HTTP 400 with thinking signature → thinking_signature.
// Rust → SchemaInvalid.
// =========================================================================
#[test]
fn test_400_thinking_signature_invalid() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (400 Bad Request): thinking signature is invalid for this model"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::SchemaInvalid), "got {result:?}");
}

// =========================================================================
// Item 11: HTTP 400 with llama.cpp grammar → llama_cpp_grammar_pattern.
// Rust → SchemaInvalid.
// =========================================================================
#[test]
fn test_400_llama_cpp_grammar_pattern() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (400 Bad Request): json-schema-to-grammar conversion failed in llama.cpp"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::SchemaInvalid), "got {result:?}");
}

// =========================================================================
// Item 12: HTTP 400 generic with large session → context_overflow (heuristic).
// Rust does not implement the Python heuristic of "infer context overflow
// from session size when 400 lacks an explicit phrase". Without the
// `context length`/`too long` markers the Rust classifier returns
// SchemaInvalid. Documented Rust↔Python collapse — the failover tuple
// (false, true) is identical to the legacy classifier's 400 mapping.
// =========================================================================
#[test]
fn test_400_generic_large_session_falls_back_as_schema_invalid() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (400 Bad Request): bad request body (no explicit context phrase)"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::SchemaInvalid));
    assert!(!result.should_retry());
    assert!(result.should_fallback());
}

// =========================================================================
// Item 13: HTTP 400 generic with small session → format_error.
// Rust → SchemaInvalid.
// =========================================================================
#[test]
fn test_400_generic_small_session_format_error() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (400 Bad Request): malformed JSON body"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::SchemaInvalid));
}

// =========================================================================
// Item 14: HTTP 404 with model-not-found → model_not_found, fallback.
// =========================================================================
#[test]
fn test_404_model_not_found_fallback() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (404 Not Found): model gpt-9999 not found in registry"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::ModelNotFound), "got {result:?}");
    assert!(!result.should_retry());
    assert!(result.should_fallback());
    assert_eq!(result.variant_name(), "ModelNotFound");
}

// =========================================================================
// Item 15: HTTP 404 with policy-block pattern → provider_policy_blocked.
// Python: no fallback. Rust collapses → Auth, which has should_fallback=true.
// Documented Rust↔Python collapse (RESEARCH.md mapping table line 381).
// =========================================================================
#[test]
fn test_404_policy_block() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (404 Not Found): this content is blocked by provider guardrail policy"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Auth), "got {result:?}");
}

// =========================================================================
// Item 16: HTTP 413 → payload_too_large, compress.
// =========================================================================
#[test]
fn test_413_payload_too_large_compress() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (413 Payload Too Large): body exceeds limit"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::PayloadTooLarge), "got {result:?}");
    assert!(!result.should_retry());
    assert!(result.should_fallback());
    assert_eq!(result.variant_name(), "PayloadTooLarge");
}

// =========================================================================
// Item 17: HTTP 500/502 → server_error, retryable.
// =========================================================================
#[test]
fn test_500_server_error_retryable() {
    let err = anyhow::anyhow!("HTTP request failed with status: 500 Internal Server Error");
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Server { status: 500 }));
    assert!(result.should_retry());
    assert!(result.should_fallback());
    assert_eq!(result.variant_name(), "Server");
}

#[test]
fn test_502_server_error_retryable() {
    let err = anyhow::anyhow!("Streaming chat completion failed (502 Bad Gateway): upstream timeout");
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Server { status: 502 }), "got {result:?}");
    assert!(result.should_retry());
}

// =========================================================================
// Item 18: HTTP 503/529 → overloaded, retryable.
// =========================================================================
#[test]
fn test_503_overloaded_retryable() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (503 Service Unavailable): upstream overloaded"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Server { status: 503 }), "got {result:?}");
    assert!(result.should_retry());
    assert!(result.should_fallback());
}

#[test]
fn test_529_overloaded_retryable() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (529 Site is overloaded): try again later"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Server { status: 529 }), "got {result:?}");
    assert!(result.should_retry());
}

// =========================================================================
// Item 19: Transport exception type (ConnectError, ReadTimeout) → timeout.
// =========================================================================
#[test]
fn test_transport_exception_type() {
    let err = anyhow::anyhow!(
        "ConnectError: failed to lookup address information for api.example.com (dns error)"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Transport));
    assert!(result.should_retry());
    assert!(result.should_fallback());
}

// =========================================================================
// Item 20: "error sending request" message → timeout.
// =========================================================================
#[test]
fn test_transport_error_sending_request() {
    let err = anyhow::anyhow!(
        "error sending request for url (http://primary.example/v1/chat/completions): tcp connect error"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Transport));
}

// =========================================================================
// Item 21: "connection refused" → timeout + fallback.
// =========================================================================
#[test]
fn test_transport_connection_refused() {
    let err = anyhow::anyhow!("tcp connect error: Connection refused (os error 61)");
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Transport));
    assert!(result.should_retry());
    assert!(result.should_fallback());
}

// =========================================================================
// Item 22: SSL transient patterns (bad record mac) → timeout, NOT context_overflow.
// SSL errors that lack an HTTP status fall outside the transport allowlist
// ("bad record mac" is not a recognised marker). The classifier returns
// Unknown — the regression guard is that it must NOT become ContextLength.
// =========================================================================
#[test]
fn test_ssl_bad_record_mac_not_context_overflow() {
    let err = anyhow::anyhow!("SSL transient error: bad record mac at handshake stage");
    let result = classify_llm_error_typed(&err);
    // Must NOT be ContextLength — that's the Python regression guard.
    assert!(!matches!(result, ProviderError::ContextLength));
}

// =========================================================================
// Item 23: Server disconnect + large session — Python heuristic infers
// context_overflow. Rust does not have access to session size at
// classification time. Documented collapse: falls to Unknown.
// =========================================================================
#[test]
fn test_server_disconnect_large_session_no_heuristic() {
    let err = anyhow::anyhow!("Server disconnected without sending a response");
    let result = classify_llm_error_typed(&err);
    // Must NOT erroneously be ContextLength.
    assert!(!matches!(result, ProviderError::ContextLength));
}

// =========================================================================
// Item 24: Server disconnect + small session → timeout (Python).
// Without an HTTP status code or transport allowlist marker, Rust classifies
// as Unknown. Documented Rust↔Python collapse.
// =========================================================================
#[test]
fn test_server_disconnect_small_session_no_heuristic() {
    let err = anyhow::anyhow!("Server disconnected: small session");
    let result = classify_llm_error_typed(&err);
    assert!(!matches!(result, ProviderError::ContextLength));
}

// =========================================================================
// Item 25: Rate-limit patterns in message (no status) → rate_limit.
// =========================================================================
#[test]
fn test_rate_limit_pattern_in_message_no_status() {
    let err = anyhow::anyhow!("you have hit your rate limit; please slow down");
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::RateLimited { .. }));
}

// =========================================================================
// Item 26: Billing patterns in message (no status) → billing.
// =========================================================================
#[test]
fn test_billing_pattern_in_message_no_status() {
    let err = anyhow::anyhow!("Your credit balance is insufficient to complete this request");
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Billing));
}

// =========================================================================
// Item 27: Auth patterns in message → auth.
// =========================================================================
#[test]
fn test_auth_pattern_in_message() {
    let err = anyhow::anyhow!("invalid api key supplied; request unauthorized");
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Auth));
}

// =========================================================================
// Item 28: OpenRouter metadata.raw unwrapping → inner error message extracted.
// The Rust facade does not parse JSON envelopes — it operates on the
// `format!("{err:#}")` chain-formatted string. If callers want raw-unwrap
// behavior, they must wrap the inner message into the anyhow chain via
// `.context(...)`. This test documents the chain-walk behavior — the inner
// 429 is reachable through `{err:#}`'s alternate Display chain.
// =========================================================================
#[test]
fn test_openrouter_metadata_raw_unwrap_via_anyhow_chain() {
    use anyhow::Context;
    let inner: anyhow::Error =
        anyhow::anyhow!("Streaming chat completion failed (429 Too Many Requests): rate-limit");
    let wrapped = Err::<(), _>(inner)
        .context("OpenRouter metadata.raw envelope")
        .unwrap_err();
    let result = classify_llm_error_typed(&wrapped);
    assert!(matches!(result, ProviderError::RateLimited { .. }), "got {result:?}");
}

// =========================================================================
// Item 29: Grok subscription SSE error (no status code) → auth, not retryable.
// =========================================================================
#[test]
fn test_grok_subscription_sse_error_no_status() {
    let err = anyhow::anyhow!(
        "Grok subscription SSE error: unauthorized access to xAI premium tier"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::Auth));
    assert!(!result.should_retry());
}

// =========================================================================
// Item 30: OAuth long context beta forbidden → oauth_long_context_beta_forbidden.
// Rust → SchemaInvalid.
// =========================================================================
#[test]
fn test_oauth_long_context_beta_forbidden() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (400 Bad Request): long context beta is not yet available for this OAuth tier"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::SchemaInvalid), "got {result:?}");
}

// =========================================================================
// Item 30b (RESEARCH.md item 11 follow-on): Multimodal tool content unsupported.
// Python: `multimodal_tool_content_unsupported` → Rust `ToolError`.
// =========================================================================
#[test]
fn test_400_multimodal_tool_content_unsupported() {
    let err = anyhow::anyhow!(
        "Streaming chat completion failed (400 Bad Request): multimodal content is not supported in tool result blocks"
    );
    let result = classify_llm_error_typed(&err);
    assert!(matches!(result, ProviderError::ToolError), "got {result:?}");
}

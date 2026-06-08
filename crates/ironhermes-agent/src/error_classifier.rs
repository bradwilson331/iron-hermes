//! Phase 36.2-03: typed `ProviderError` taxonomy + canonical classifier.
//!
//! This module replaces the `(bool, bool)` shape of `classify_llm_error`
//! with a typed enum, additively (D-ERR-03). The legacy
//! `AgentLoop::classify_llm_error` in `agent_loop.rs` becomes a one-line
//! facade calling `classify_llm_error_typed(err).into()` — every existing
//! call site (and every existing test at agent_loop.rs:2520-2724) keeps
//! working byte-identically.
//!
//! Plan 06 (RateLimitTracker) consumes [`ProviderError::RateLimited`] —
//! destructuring the `retry_after` for state seeding.
//!
//! Plan 07 (`usage_events` writer) stores [`ProviderError::variant_name`] in
//! the `error_kind` TEXT column — a bounded `&'static str` set means HTTP
//! response bodies and PII cannot leak via this code path.
//!
//! The two existing helpers (`extract_http_status`, `is_transport_failure`)
//! stay in `agent_loop.rs` so that the static-grep regression in
//! `tests/invariants_27_1_4_1_1.rs` (PROV-07 transport-failure helper) keeps
//! passing without modification. Their visibility is widened to
//! `pub(crate)` here so this module can call into them.

use std::time::Duration;

use crate::agent_loop::AgentLoop;

/// Typed provider-error taxonomy (D-ERR-01).
///
/// One canonical source of truth consumed by:
///   * [`crate::agent_loop::AgentLoop::classify_llm_error`] facade (returns `(bool, bool)`)
///   * Phase 36.2 Plan 06 — RateLimitTracker seeds state from
///     `RateLimited { retry_after }`
///   * Phase 36.2 Plan 07 — `usage_events.error_kind` stores
///     `variant_name(): &'static str`
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderError {
    /// HTTP 429, OR explicit `rate limit` / `too many requests` message
    /// signal in the absence of an HTTP status. `retry_after` is `Some` only
    /// when a `retry-after` (or `retry_after`) field is present in the error
    /// chain string.
    RateLimited { retry_after: Option<Duration> },
    /// HTTP 401 / 403, OR explicit `unauthorized` / `invalid api key` /
    /// `authentication` substring. Also catches Anthropic / OpenRouter
    /// policy-block 404s.
    Auth,
    /// HTTP 402, OR explicit `billing` / `credit balance` / `insufficient
    /// credit` substring. Distinct from `RateLimited` — the user is out of
    /// money, not requests.
    Billing,
    /// HTTP 400 with context-overflow markers (`context length`, `too long`,
    /// `maximum context length`).
    ContextLength,
    /// HTTP 5xx — provider down.
    Server { status: u16 },
    /// Connection refused / DNS failure / TCP connect error — provider
    /// unreachable. Driven by `AgentLoop::is_transport_failure` (the existing
    /// allowlist).
    Transport,
    /// HTTP 400 with schema / signature / grammar / OAuth-beta forbidden
    /// markers; HTTP 404 generic (non-model, non-policy).
    SchemaInvalid,
    /// HTTP 400 with `multimodal` + `tool` markers — the provider rejected
    /// the tool call payload.
    ToolError,
    /// HTTP 404 with `model … not found` markers.
    ModelNotFound,
    /// HTTP 413 — the request body exceeded provider limits (image too
    /// large, payload too large).
    PayloadTooLarge,
    /// Catch-all — unrecognised error.
    Unknown,
}

impl ProviderError {
    /// Should the caller retry the same request against the same provider?
    ///
    /// The truth table here MUST match the legacy classifier at
    /// `agent_loop.rs:742-761` for every input fed to the existing 12+
    /// tests in `fallback_tests` (lines 2520-2724) — see Pitfall guard in
    /// PLAN.md.
    pub fn should_retry(&self) -> bool {
        match self {
            Self::RateLimited { .. } => true, // 429 → legacy (true, true)
            Self::Auth => false,              // 401 / 403 → legacy (false, true)
            Self::Billing => false,           // 402 → no retry on a no-funds wallet
            Self::ContextLength => false,     // 400-context → permanent client-error
            Self::Server { .. } => true,      // 5xx → legacy (true, true)
            Self::Transport => true,          // legacy (true, transport-failure?)
            Self::SchemaInvalid => false,     // 400 → legacy (false, true)
            Self::ToolError => false,
            Self::ModelNotFound => false, // 404 → legacy (false, true)
            Self::PayloadTooLarge => false, // 413 — compress, do not retry
            Self::Unknown => true,        // legacy fallback: (true, false) → retry
        }
    }

    /// Should the caller swap to the configured fallback provider?
    pub fn should_fallback(&self) -> bool {
        match self {
            Self::RateLimited { .. } => true, // 429 → legacy (true, true)
            Self::Auth => true,
            Self::Billing => true,
            Self::ContextLength => true,
            Self::Server { .. } => true,
            Self::Transport => true,
            Self::SchemaInvalid => true, // legacy 400/404 → (false, true)
            Self::ToolError => false,
            Self::ModelNotFound => true,
            Self::PayloadTooLarge => true,
            Self::Unknown => false, // legacy unknown: (true, false)
        }
    }

    /// Stable identity string for storage in `usage_events.error_kind`.
    ///
    /// Bounded to a compile-time set of 11 `&'static str` values — guarantees
    /// no HTTP body, header value, or PII can leak via this column. Plan 07
    /// stores `Some(provider_error.variant_name().to_string())`.
    pub fn variant_name(&self) -> &'static str {
        match self {
            Self::RateLimited { .. } => "RateLimited",
            Self::Auth => "Auth",
            Self::Billing => "Billing",
            Self::ContextLength => "ContextLength",
            Self::Server { .. } => "Server",
            Self::Transport => "Transport",
            Self::SchemaInvalid => "SchemaInvalid",
            Self::ToolError => "ToolError",
            Self::ModelNotFound => "ModelNotFound",
            Self::PayloadTooLarge => "PayloadTooLarge",
            Self::Unknown => "Unknown",
        }
    }
}

impl From<ProviderError> for (bool, bool) {
    fn from(e: ProviderError) -> Self {
        (e.should_retry(), e.should_fallback())
    }
}

/// Canonical typed classifier (D-ERR-03).
///
/// Reuses `AgentLoop::extract_http_status` and `AgentLoop::is_transport_failure`
/// in-place — zero rewrite of the existing helpers. Maps HTTP codes +
/// transport patterns + provider-specific message patterns to enum variants.
pub fn classify_llm_error_typed(err: &anyhow::Error) -> ProviderError {
    // Alternate Display walks the full anyhow context chain joined with ": ".
    // Plain Display only surfaces the outermost context — production errors
    // are wrapped at `agent_loop.rs:1030` with `.context("Streaming LLM call failed")`,
    // hiding the underlying `(400 Bad Request)` from the substring scan.
    let err_str = format!("{err:#}");

    if let Some(code) = AgentLoop::extract_http_status(&err_str) {
        return match code {
            429 => ProviderError::RateLimited {
                retry_after: parse_retry_after(&err_str),
            },
            401 | 403 => ProviderError::Auth,
            402 => ProviderError::Billing,
            400 => classify_400_subcases(&err_str),
            404 => classify_404_subcases(&err_str),
            413 => {
                // 413 can be context-overflow OR payload-too-large. The legacy
                // classifier mapped 413 to (false, true) which is the same
                // tuple for both ContextLength and PayloadTooLarge — pick
                // PayloadTooLarge for the typed enum since the wire-level
                // signal (HTTP 413) is "request body too large".
                ProviderError::PayloadTooLarge
            }
            500 | 502 | 503 | 504 | 529 => ProviderError::Server { status: code },
            _ => ProviderError::Unknown,
        };
    }

    // No HTTP status — check transport markers via the existing helper.
    if AgentLoop::is_transport_failure(&err_str) {
        return ProviderError::Transport;
    }

    // No status, no transport marker — check message-pattern fallbacks
    // (covers provider-specific signals like Grok's SSE subscription error).
    let lower = err_str.to_lowercase();
    if lower.contains("rate limit")
        || lower.contains("rate_limit")
        || lower.contains("too many requests")
    {
        return ProviderError::RateLimited {
            retry_after: parse_retry_after(&err_str),
        };
    }
    if lower.contains("billing")
        || lower.contains("credit balance")
        || lower.contains("insufficient credit")
    {
        return ProviderError::Billing;
    }
    if lower.contains("unauthorized")
        || lower.contains("invalid api key")
        || lower.contains("authentication")
    {
        return ProviderError::Auth;
    }

    ProviderError::Unknown
}

/// Best-effort extraction of a `retry-after` field from a free-form error
/// chain string. No regex — simple needle search + ASCII-digit parse.
fn parse_retry_after(err_str: &str) -> Option<Duration> {
    let lower = err_str.to_lowercase();
    for needle in [
        "retry-after:",
        "retry_after:",
        "retry after:",
        "retry-after ",
    ] {
        if let Some(idx) = lower.find(needle) {
            // Use byte indices into the lowercased haystack to find the
            // numeric tail; ASCII so byte == char alignment is preserved.
            let tail = &lower[idx + needle.len()..];
            let secs_str: String = tail
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(n) = secs_str.parse::<u64>() {
                // CR-10: clamp to 24h. An adversarial server returning
                // Retry-After: 9223372036854775000 would otherwise produce a
                // Duration that overflows downstream SystemTime arithmetic.
                let capped = n.min(86_400);
                return Some(Duration::from_secs(capped));
            }
        }
    }
    None
}

/// Disambiguate HTTP 400 by message contents (Anthropic / OpenRouter /
/// llama.cpp / OAuth beta sub-cases).
fn classify_400_subcases(err_str: &str) -> ProviderError {
    let lower = err_str.to_lowercase();
    // Context-overflow patterns
    if lower.contains("context length")
        || lower.contains("maximum context length")
        || (lower.contains("context") && lower.contains("too long"))
        || (lower.contains("context") && lower.contains("overflow"))
    {
        return ProviderError::ContextLength;
    }
    // Image-too-large (Anthropic vision)
    if lower.contains("image") && (lower.contains("too large") || lower.contains("size limit")) {
        return ProviderError::PayloadTooLarge;
    }
    // Multimodal tool content rejected
    if lower.contains("multimodal") && lower.contains("tool") {
        return ProviderError::ToolError;
    }
    // Anthropic thinking-signature
    if lower.contains("thinking") && lower.contains("signature") {
        return ProviderError::SchemaInvalid;
    }
    // llama.cpp grammar / json schema → grammar errors
    if lower.contains("json-schema-to-grammar")
        || lower.contains("llama.cpp")
        || lower.contains("grammar")
    {
        return ProviderError::SchemaInvalid;
    }
    // OAuth long-context-beta forbidden (returns 400)
    if lower.contains("long context") && lower.contains("beta") {
        return ProviderError::SchemaInvalid;
    }
    // Fall-through 400 (format errors, invalid model IDs that OpenRouter
    // returns as 400, etc.) — legacy classifier said (false, true), the
    // typed enum represents this as SchemaInvalid.
    ProviderError::SchemaInvalid
}

/// Disambiguate HTTP 404 by message contents (model-not-found vs
/// policy-blocked vs generic).
fn classify_404_subcases(err_str: &str) -> ProviderError {
    let lower = err_str.to_lowercase();
    if lower.contains("model") && lower.contains("not found") {
        return ProviderError::ModelNotFound;
    }
    if lower.contains("policy") || lower.contains("guardrail") || lower.contains("blocked") {
        // Provider-policy-blocked maps to Auth in the Python failover table.
        return ProviderError::Auth;
    }
    ProviderError::SchemaInvalid
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    // -----------------------------------------------------------------
    // Parity guard: every legacy `classify_llm_error` test input must map
    // to the same `(bool, bool)` via the new typed classifier + `.into()`.
    // -----------------------------------------------------------------

    fn legacy(err: &anyhow::Error) -> (bool, bool) {
        classify_llm_error_typed(err).into()
    }

    #[test]
    fn parity_429_returns_true_true() {
        let err = anyhow!("HTTP request failed with status: 429 Too Many Requests");
        assert_eq!(legacy(&err), (true, true));
    }

    #[test]
    fn parity_401_returns_false_true() {
        let err = anyhow!("HTTP request failed with status: 401 Unauthorized");
        assert_eq!(legacy(&err), (false, true));
    }

    #[test]
    fn parity_other_error_returns_true_false() {
        let err = anyhow!("unexpected end of JSON input");
        assert_eq!(legacy(&err), (true, false));
    }

    #[test]
    fn parity_transport_request_send_marker() {
        let err = anyhow!(
            "Streaming LLM call failed: Failed to send streaming request: \
             error sending request for url (http://localhost:11434/v1/chat/completions): \
             tcp connect error: Connection refused (os error 61)"
        );
        assert_eq!(legacy(&err), (true, true));
    }

    #[test]
    fn parity_transport_connection_refused() {
        let err = anyhow!("tcp connect error: Connection refused (os error 61)");
        assert_eq!(legacy(&err), (true, true));
    }

    #[test]
    fn parity_transport_connect_timeout() {
        let err = anyhow!(
            "error sending request for url (http://localhost:11434/...): \
             tcp connect error: Operation timed out (os error 60)"
        );
        assert_eq!(legacy(&err), (true, true));
    }

    #[test]
    fn parity_transport_dns_failure() {
        let err = anyhow!(
            "error sending request for url (http://nope.invalid/...): \
             dns error: failed to lookup address information: \
             nodename nor servname provided, or not known"
        );
        assert_eq!(legacy(&err), (true, true));
    }

    #[test]
    fn parity_transport_connection_reset() {
        let err = anyhow!("Connection reset by peer (os error 54)");
        assert_eq!(legacy(&err), (true, true));
    }

    #[test]
    fn parity_sse_read_timeout_not_transport() {
        // "timed out" alone (without "operation timed out") is not on the
        // transport allowlist; legacy returns (true, false).
        let err = anyhow!("SSE stream read timed out after 60s");
        assert_eq!(legacy(&err), (true, false));
    }

    #[test]
    fn parity_walks_anyhow_context_chain() {
        use anyhow::Context;
        let inner: anyhow::Error =
            anyhow!("Streaming chat completion failed (400 Bad Request): {{}}");
        let wrapped = Err::<(), _>(inner)
            .context("Streaming LLM call failed")
            .unwrap_err();
        assert_eq!(legacy(&wrapped), (false, true));
    }

    #[test]
    fn parity_production_error_format_400() {
        let err = anyhow!(
            "Streaming chat completion failed (400 Bad Request): \
             {{\"error\":{{\"message\":\"openai/sgpt-4o-mini is not a valid model ID\"}}}}"
        );
        assert_eq!(legacy(&err), (false, true));
    }

    #[test]
    fn parity_production_error_format_404() {
        let err = anyhow!("Streaming chat completion failed (404 Not Found): {{}}");
        let (_, fb) = legacy(&err);
        assert!(fb, "404 must trigger fallback");
    }

    #[test]
    fn parity_production_error_format_429() {
        let err = anyhow!("Streaming chat completion failed (429 Too Many Requests): {{}}");
        assert_eq!(legacy(&err), (true, true));
    }

    #[test]
    fn parity_production_error_format_500() {
        let err = anyhow!("Streaming chat completion failed (500 Internal Server Error): {{}}");
        assert_eq!(legacy(&err), (true, true));
    }

    // -----------------------------------------------------------------
    // Typed-variant assertions (the new contract).
    // -----------------------------------------------------------------

    #[test]
    fn typed_429_is_rate_limited() {
        let err = anyhow!("HTTP request failed with status: 429 Too Many Requests");
        assert!(matches!(
            classify_llm_error_typed(&err),
            ProviderError::RateLimited { retry_after: None }
        ));
    }

    #[test]
    fn typed_429_with_retry_after_header_parses_duration() {
        let err =
            anyhow!("HTTP request failed with status: 429 Too Many Requests; retry-after: 60");
        match classify_llm_error_typed(&err) {
            ProviderError::RateLimited { retry_after } => {
                assert_eq!(retry_after, Some(Duration::from_secs(60)));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn typed_401_is_auth() {
        let err = anyhow!("HTTP request failed with status: 401 Unauthorized");
        assert_eq!(classify_llm_error_typed(&err), ProviderError::Auth);
    }

    #[test]
    fn typed_403_is_auth() {
        let err = anyhow!("HTTP request failed with status: 403 Forbidden");
        assert_eq!(classify_llm_error_typed(&err), ProviderError::Auth);
    }

    #[test]
    fn typed_500_is_server() {
        let err = anyhow!("HTTP request failed with status: 500 Internal Server Error");
        assert_eq!(
            classify_llm_error_typed(&err),
            ProviderError::Server { status: 500 }
        );
    }

    #[test]
    fn typed_502_is_server() {
        let err = anyhow!("HTTP request failed with status: 502 Bad Gateway");
        assert_eq!(
            classify_llm_error_typed(&err),
            ProviderError::Server { status: 502 }
        );
    }

    #[test]
    fn typed_503_is_server() {
        let err = anyhow!("HTTP request failed with status: 503 Service Unavailable");
        assert_eq!(
            classify_llm_error_typed(&err),
            ProviderError::Server { status: 503 }
        );
    }

    #[test]
    fn typed_413_is_payload_too_large() {
        let err = anyhow!("HTTP request failed with status: 413 Payload Too Large");
        assert_eq!(
            classify_llm_error_typed(&err),
            ProviderError::PayloadTooLarge
        );
    }

    #[test]
    fn typed_transport_is_transport() {
        let err = anyhow!("error sending request for url: connection refused");
        assert_eq!(classify_llm_error_typed(&err), ProviderError::Transport);
    }

    #[test]
    fn typed_unknown_is_unknown() {
        let err = anyhow!("unexpected end of JSON input");
        assert_eq!(classify_llm_error_typed(&err), ProviderError::Unknown);
    }

    #[test]
    fn variant_name_returns_bounded_string() {
        // Stability + bounded-set contract — `usage_events.error_kind` storage
        // (Plan 07) relies on this returning compile-time strings, never
        // payload-derived text.
        assert_eq!(
            ProviderError::RateLimited { retry_after: None }.variant_name(),
            "RateLimited"
        );
        assert_eq!(ProviderError::Auth.variant_name(), "Auth");
        assert_eq!(ProviderError::Billing.variant_name(), "Billing");
        assert_eq!(ProviderError::ContextLength.variant_name(), "ContextLength");
        assert_eq!(
            ProviderError::Server { status: 500 }.variant_name(),
            "Server"
        );
        assert_eq!(ProviderError::Transport.variant_name(), "Transport");
        assert_eq!(ProviderError::SchemaInvalid.variant_name(), "SchemaInvalid");
        assert_eq!(ProviderError::ToolError.variant_name(), "ToolError");
        assert_eq!(ProviderError::ModelNotFound.variant_name(), "ModelNotFound");
        assert_eq!(
            ProviderError::PayloadTooLarge.variant_name(),
            "PayloadTooLarge"
        );
        assert_eq!(ProviderError::Unknown.variant_name(), "Unknown");
    }

    #[test]
    fn from_provider_error_for_tuple_round_trip() {
        let (r, f): (bool, bool) = ProviderError::RateLimited { retry_after: None }.into();
        assert_eq!((r, f), (true, true));
    }
}

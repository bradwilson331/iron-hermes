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
use ironhermes_core::provider::canonical_api_key_env_name;

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

/// Identity of the failure that CAUSED the agent loop to fail over to its
/// fallback provider (quick task 260819-rkz, RKZ-B).
///
/// Captured at failover time because both the primary error value and
/// `AgentLoop::provider_name` are gone by the time the fallback chain
/// eventually gives up too: `err` is dropped when the retry/fallback loop
/// `continue`s past the failover branch, and `provider_name` is overwritten
/// with the fallback's own name a few lines after failover fires. Without
/// this capture, a terminal report can only describe the LAST (fallback)
/// failure, laundering the real root cause into an unrelated symptom (e.g.
/// "connection refused" against a fallback endpoint, when the actual cause
/// was a missing primary API key).
#[derive(Debug, Clone, PartialEq)]
pub struct FallbackRootCause {
    /// Typed classification of the primary provider's failure.
    pub kind: ProviderError,
    /// Canonical name of the provider that produced the primary failure.
    pub provider: String,
    /// The primary error rendered with alternate (`{:#}`) `Display` — the
    /// full anyhow context chain joined with `": "`.
    pub detail: String,
}

/// Upper bound, in `char`s (never bytes — provider error bodies may contain
/// multi-byte text), on how much of a [`FallbackRootCause::detail`] is
/// echoed into a composed chain-failure message.
///
/// The composed string reaches a user-visible chat bubble via
/// `iron_hermes_ui/src/server/ws.rs`'s `Agent error: {e:#}` rendering, so an
/// unbounded provider response body must never be pasted there verbatim
/// (T-RKZ-02).
const ROOT_CAUSE_DETAIL_CHAR_BUDGET: usize = 400;

/// Placeholder rendered in place of a blank-or-whitespace provider name so
/// the composed message never contains a bare empty pair of quotes.
const BLANK_PROVIDER_PLACEHOLDER: &str = "<unknown provider>";

/// Appended to a truncated detail so the reader knows text was cut, rather
/// than believing the primary error was that short.
const DETAIL_TRUNCATION_MARKER: &str = " …[truncated]";

/// Truncate `s` to at most `max_chars` **characters** (never a byte index —
/// a byte-index cut of multi-byte text panics), appending
/// [`DETAIL_TRUNCATION_MARKER`] only when truncation actually occurred.
fn truncate_chars_with_marker(s: &str, max_chars: usize) -> String {
    let mut out: String = s.chars().take(max_chars).collect();
    if s.chars().count() > max_chars {
        out.push_str(DETAIL_TRUNCATION_MARKER);
    }
    out
}

fn provider_display_name(name: &str) -> &str {
    if name.trim().is_empty() {
        BLANK_PROVIDER_PLACEHOLDER
    } else {
        name
    }
}

/// Token that introduces a remediation-hint parenthetical in the composed
/// chain-failure message. Tests assert its ABSENCE to prove a root kind with
/// no hint (e.g. `SchemaInvalid`) produces no remediation segment at all —
/// not an empty-but-punctuated one.
const REMEDIATION_HINT_INTRODUCER: &str = "fix:";

/// Build an operator-facing remediation hint for a root-cause kind (quick
/// task 260819-rkz, RKZ-B enrichment). Returns `Some` for exactly three
/// kinds where a concrete, actionable next step exists; `None` for
/// everything else (including `RateLimited`/`Server`/`Transport`, which
/// resolve themselves, and `SchemaInvalid`/`ContextLength`/`ToolError`/
/// `PayloadTooLarge`/`Unknown`, which have no single fix to name).
///
/// Security constraint (T-RKZ-01, non-negotiable): built EXCLUSIVELY from
/// the provider NAME and a compile-time `&'static str` env-var NAME from
/// [`ironhermes_core::provider::canonical_api_key_env_name`]. Never reads
/// `std::env` and never touches `api_key`, `api_key_for_usage_tracking`, or
/// any resolved key value.
fn remediation_hint(kind: &ProviderError, provider: &str) -> Option<String> {
    match kind {
        ProviderError::Auth => Some(match canonical_api_key_env_name(provider) {
            Some(env_name) => format!(
                "set {env_name} in the .env file under the IronHermes home directory \
                 (or configure providers.{provider}.api_key_env)"
            ),
            None => format!(
                "declare providers.{provider}.api_key_env in config.yaml and supply that \
                 environment variable — '{provider}' is not a built-in provider, so no \
                 canonical variable name is known"
            ),
        }),
        ProviderError::Billing => Some(format!(
            "the '{provider}' account has no remaining credit — add funds or switch providers"
        )),
        ProviderError::ModelNotFound => Some(format!(
            "the model id configured for '{provider}' was rejected by that provider — check \
             providers.{provider}.default_model"
        )),
        _ => None,
    }
}

/// Compose a single-line description of a provider-chain failure whose
/// FIRST named entity is the ROOT cause (the primary provider's failure),
/// not the fallback's own — usually more confusing — symptom. This is the
/// literal fix for RKZ-B: today the fallback's own error leads and the
/// primary's non-retryable failure is discarded entirely.
///
/// Pure: no logging, no env reads, no I/O, no `self`. Segment order: a
/// fixed lead-in identifying this as a provider-chain failure; the root
/// cause's [`ProviderError::variant_name`]; the primary provider name in
/// quotes; an optional remediation-hint parenthetical from
/// [`remediation_hint`] (nothing at all when the kind has no hint — no
/// stray separator, no empty parentheses); the char-truncated primary
/// detail; and only then the current (fallback) provider name, framed as
/// the secondary failure whose own error follows via the caller's
/// `anyhow::Context::context` wrapping.
pub fn describe_provider_chain_failure(root: &FallbackRootCause, current_provider: &str) -> String {
    let primary_name = provider_display_name(&root.provider);
    let current_name = provider_display_name(current_provider);
    let truncated_detail = truncate_chars_with_marker(&root.detail, ROOT_CAUSE_DETAIL_CHAR_BUDGET);

    let hint_segment = remediation_hint(&root.kind, primary_name)
        .map(|hint| format!(" ({REMEDIATION_HINT_INTRODUCER} {hint})"))
        .unwrap_or_default();

    format!(
        "provider chain failed: primary provider '{primary}' failed with \
         {kind}{hint} — {detail} — failover then activated fallback \
         provider '{current}', which also failed",
        primary = primary_name,
        kind = root.kind.variant_name(),
        hint = hint_segment,
        detail = truncated_detail,
        current = current_name,
    )
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

// ---------------------------------------------------------------------------
// Quick task 260819-rkz: FallbackRootCause / describe_provider_chain_failure
// ---------------------------------------------------------------------------

#[cfg(test)]
mod chain_failure_tests {
    use super::*;

    fn root(kind: ProviderError, provider: &str, detail: &str) -> FallbackRootCause {
        FallbackRootCause {
            kind,
            provider: provider.to_string(),
            detail: detail.to_string(),
        }
    }

    #[test]
    fn root_leads_ahead_of_current_provider() {
        let r = root(ProviderError::Auth, "openrouter", "401 Unauthorized");
        let msg = describe_provider_chain_failure(&r, "ollama");
        let primary_pos = msg
            .find("openrouter")
            .expect("composed message must mention the primary provider");
        let current_pos = msg
            .find("ollama")
            .expect("composed message must mention the current (fallback) provider");
        assert!(
            primary_pos < current_pos,
            "root cause provider must lead the fallback provider in the composed \
             message; got primary at {primary_pos}, current at {current_pos}: {msg}"
        );
    }

    #[test]
    fn kind_is_named_via_variant_name() {
        let r = root(ProviderError::Auth, "openrouter", "401 Unauthorized");
        let msg = describe_provider_chain_failure(&r, "ollama");
        assert!(
            msg.contains(ProviderError::Auth.variant_name()),
            "composed message must name the root kind via variant_name(): {msg}"
        );
    }

    #[test]
    fn blank_provider_names_degrade_gracefully() {
        let r = root(ProviderError::Auth, "", "401 Unauthorized");
        let msg = describe_provider_chain_failure(&r, "");
        assert!(
            !msg.contains("''"),
            "blank provider names must not render as an empty quoted string: {msg}"
        );
        assert!(
            msg.matches(BLANK_PROVIDER_PLACEHOLDER).count() >= 2,
            "both blank primary and blank current provider names must render as the \
             placeholder token: {msg}"
        );
    }

    #[test]
    fn detail_is_bounded_and_multibyte_safe() {
        let long_detail = "x".repeat(5000);
        let r = root(ProviderError::Auth, "openrouter", &long_detail);
        let msg = describe_provider_chain_failure(&r, "ollama");
        assert!(
            msg.len() < 5000,
            "composed message must be materially shorter than an unbounded 5000-char \
             detail: got {} chars",
            msg.len()
        );

        // Multi-byte characters must not panic on truncation (byte-index cuts of
        // multi-byte UTF-8 panic; char-based truncation does not).
        let multibyte_detail = "é".repeat(5000);
        let r = root(ProviderError::Auth, "openrouter", &multibyte_detail);
        let _ = describe_provider_chain_failure(&r, "ollama");

        let multibyte_detail_cjk = "日".repeat(5000);
        let r = root(ProviderError::Auth, "openrouter", &multibyte_detail_cjk);
        let _ = describe_provider_chain_failure(&r, "ollama");
    }

    #[test]
    fn original_detail_survives_in_composed_message() {
        let detail = "connection refused while contacting openrouter.ai endpoint, retry later";
        let r = root(ProviderError::Auth, "openrouter", detail);
        let msg = describe_provider_chain_failure(&r, "ollama");
        let prefix: String = detail.chars().take(50).collect();
        assert!(
            msg.contains(&prefix),
            "composed message must retain the primary detail's own text, not just a \
             label: expected prefix {prefix:?} in {msg}"
        );
    }

    // -----------------------------------------------------------------
    // Task 3 (RKZ-B enrichment): remediation hint.
    // -----------------------------------------------------------------

    #[test]
    fn auth_root_from_canonical_provider_names_its_env_var() {
        let r = root(ProviderError::Auth, "openrouter", "401 Unauthorized");
        let msg = describe_provider_chain_failure(&r, "ollama");
        assert!(
            msg.contains("OPENROUTER_API_KEY"),
            "an Auth root from a canonical provider must name that provider's \
             canonical env var: {msg}"
        );
    }

    #[test]
    fn auth_root_from_custom_provider_points_at_config_key_not_a_guessed_var() {
        let r = root(ProviderError::Auth, "my-custom-llm", "401 Unauthorized");
        let msg = describe_provider_chain_failure(&r, "ollama");
        assert!(
            msg.contains("my-custom-llm"),
            "the composed message must still name the custom provider: {msg}"
        );
        assert!(
            msg.contains("api_key_env"),
            "an Auth root from an unrecognized custom provider must point the \
             operator at the per-provider config key, not a guessed env var \
             name: {msg}"
        );
    }

    #[test]
    fn schema_invalid_root_has_no_remediation_segment() {
        let r = root(
            ProviderError::SchemaInvalid,
            "openrouter",
            "400 Bad Request",
        );
        let msg = describe_provider_chain_failure(&r, "ollama");
        assert!(
            !msg.contains(REMEDIATION_HINT_INTRODUCER),
            "a SchemaInvalid root must produce NO remediation segment — the \
             introducer token must be entirely absent, not empty-but-punctuated: {msg}"
        );
    }

    #[test]
    fn root_leads_ordering_holds_with_hint_present() {
        let r = root(ProviderError::Auth, "openrouter", "401 Unauthorized");
        let msg = describe_provider_chain_failure(&r, "ollama");
        let primary_pos = msg
            .find("openrouter")
            .expect("composed message must mention the primary provider");
        let current_pos = msg
            .find("ollama")
            .expect("composed message must mention the current (fallback) provider");
        assert!(
            primary_pos < current_pos,
            "the root-leads ordering guarantee must still hold once the \
             remediation hint is present: primary at {primary_pos}, current at \
             {current_pos}: {msg}"
        );
    }
}

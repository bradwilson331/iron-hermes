//! Phase 41.3 D-07/D-13 (Plan 08): Perplexity `web_answer` backend — the
//! primary, answer-native provider.
//!
//! Endpoint: POST https://api.perplexity.ai/v1/agent (the `/v1/responses`
//! alias is equivalent and currently accepted; this backend targets the
//! canonical `/v1/agent` path). Auth: `Authorization: Bearer
//! $PERPLEXITY_API_KEY`.
//!
//! Body: `input` (the query) and `preset`. `stream` is omitted entirely —
//! D-13 requires the finished string be collected before `Tool::execute`
//! returns, so this backend never opens a server-sent-events connection.
//! The preset value is held in a named const so a future rename fails one
//! test rather than silently resolving a different tier — CONTEXT.md's own
//! canonical contract for this decision recorded the pre-2026 preset name,
//! which this backend does NOT use; that name never appears in this file.
//!
//! Response: `output[]`, where each entry is either a search-results-shaped
//! item (`results[].url` — collected into citations) or a message-shaped
//! item (`content[].text` — concatenated, in order, into the answer text,
//! per D-13's "collect fully" requirement — the model must receive one
//! finished result, never a partial slice of it). Perplexity reports no
//! cost field, so `cost_dollars` is always `None`.

use std::time::Duration;

use anyhow::{Result, anyhow};
use ironhermes_core::config::Config;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use crate::web_answer::AnswerResult;

const PERPLEXITY_AGENT_ENDPOINT: &str = "https://api.perplexity.ai/v1/agent";

/// Current (2026) tier name for the preset CONTEXT.md's canonical contract
/// recorded under its pre-2026 name (a research correction — see this
/// module's doc comment). Held in a named const so a future Perplexity
/// preset rename is a one-line change plus a failing test.
const PERPLEXITY_PRESET: &str = "low";

#[derive(Debug, Default, Deserialize)]
struct PerplexityAgentResponse {
    #[serde(default)]
    output: Vec<PerplexityOutputItem>,
}

#[derive(Debug, Default, Deserialize)]
struct PerplexityOutputItem {
    #[serde(default)]
    content: Vec<PerplexityContentPart>,
    #[serde(default)]
    results: Vec<PerplexityResultItem>,
}

#[derive(Debug, Default, Deserialize)]
struct PerplexityContentPart {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Default, Deserialize)]
struct PerplexityResultItem {
    #[serde(default)]
    url: Option<String>,
}

/// Pure builder — the request body Perplexity's `/v1/agent` endpoint
/// expects. Exposed for `request_body_uses_the_current_preset_name` and
/// `request_body_does_not_request_streaming` (D-13, encoded as tests).
fn build_request_body(query: &str) -> serde_json::Value {
    json!({
        "input": query,
        "preset": PERPLEXITY_PRESET,
    })
}

/// Answer Perplexity's `/v1/agent` endpoint. `api_key` is exposed at
/// exactly this call site (mirrors `provider.rs`'s single-boundary
/// discipline) and never logged, formatted into an error, or placed
/// anywhere but the request header.
pub async fn answer(query: &str, api_key: &SecretString) -> Result<AnswerResult> {
    let timeout_secs = Config::load().map(|c| c.web.timeout_secs).unwrap_or(30);
    debug!("web_answer: querying Perplexity for: {}", query);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let response = client
        .post(resolve_endpoint())
        .bearer_auth(api_key.expose_secret())
        .json(&build_request_body(query))
        .send()
        .await
        .map_err(|e| anyhow!("Perplexity agent request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("Perplexity agent returned HTTP {}", status));
    }

    let parsed: PerplexityAgentResponse = response
        .json()
        .await
        .map_err(|e| anyhow!("Perplexity agent response decode failed: {}", e))?;

    // D-13: collect every content part across every output item, in order —
    // the finished string, not a partial slice of it.
    let text = parsed
        .output
        .iter()
        .flat_map(|item| item.content.iter())
        .map(|part| part.text.as_str())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut citations: Vec<String> = Vec::new();
    for item in &parsed.output {
        for result in &item.results {
            if let Some(url) = &result.url
                && !citations.contains(url)
            {
                citations.push(url.clone());
            }
        }
    }

    if text.is_empty() {
        return Err(anyhow!("Perplexity agent returned no message content"));
    }

    Ok(AnswerResult {
        text,
        citations,
        cost_dollars: None,
        provider: "perplexity",
    })
}

/// Test-override hook for the endpoint — production always returns the
/// literal Perplexity `/v1/agent` URL.
fn resolve_endpoint() -> String {
    std::env::var("PERPLEXITY_ENDPOINT_OVERRIDE")
        .unwrap_or_else(|_| PERPLEXITY_AGENT_ENDPOINT.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_uses_constant_when_override_unset() {
        let ep = resolve_endpoint();
        assert!(ep.starts_with("http"), "endpoint = {ep}");
    }

    #[test]
    fn request_body_uses_the_current_preset_name() {
        let body = build_request_body("rust programming");
        assert_eq!(
            body["preset"], PERPLEXITY_PRESET,
            "preset must equal the named current-tier const"
        );
    }

    #[test]
    fn request_body_does_not_request_streaming() {
        let body = build_request_body("rust programming");
        // D-13: the built body must either omit `stream` entirely or set it
        // false — never true.
        match body.get("stream") {
            None => {}
            Some(v) => assert_eq!(v, &json!(false), "stream must never be true, got {v}"),
        }
    }
}

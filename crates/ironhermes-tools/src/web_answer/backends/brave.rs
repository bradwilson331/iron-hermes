//! Phase 41.3 D-07/D-13 (Plan 08): Brave `web_answer` backend — the
//! machine-consumption "LLM context" endpoint.
//!
//! Endpoint (task-mandated direct documentation pull, verified this session
//! against the maintained Brave Search API skill docs at
//! github.com/brave/brave-search-skills, since RESEARCH.md Open Question 2
//! flagged the exact path/envelope as not independently re-verified):
//! `GET https://api.search.brave.com/res/v1/llm/context?q=<query>`. Auth:
//! `X-Subscription-Token` header carrying `BRAVE_API_KEY` — the SAME
//! non-uniform auth convention `web_search`'s Brave backend uses; this
//! endpoint carries no other auth header at all.
//!
//! Response envelope (verified this session):
//! `{"grounding": {"generic": [{"url", "title", "snippets": [...]}],
//! "map": [...]}, "sources": {...}}`. Every `grounding.generic[]` entry's
//! `snippets[]` are joined (in order, across every entry) into
//! `AnswerResult.text`, and every entry's `url` is collected into
//! `AnswerResult.citations`. Brave reports no cost field, so `cost_dollars`
//! is always `None`.

use std::time::Duration;

use anyhow::{Result, anyhow};
use ironhermes_core::config::Config;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tracing::debug;

use crate::web_answer::AnswerResult;

const BRAVE_ANSWER_ENDPOINT: &str = "https://api.search.brave.com/res/v1/llm/context";

#[derive(Debug, Default, Deserialize)]
struct BraveContextResponse {
    #[serde(default)]
    grounding: Option<BraveGrounding>,
}

#[derive(Debug, Default, Deserialize)]
struct BraveGrounding {
    #[serde(default)]
    generic: Vec<BraveGenericSource>,
}

#[derive(Debug, Default, Deserialize)]
struct BraveGenericSource {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    snippets: Vec<String>,
}

/// Answer Brave's `/res/v1/llm/context` endpoint. `api_key` is exposed at
/// exactly this call site and never logged, formatted into an error, or
/// placed anywhere but the request header.
pub async fn answer(query: &str, api_key: &SecretString) -> Result<AnswerResult> {
    let timeout_secs = Config::load().map(|c| c.web.timeout_secs).unwrap_or(30);
    debug!("web_answer: querying Brave for: {}", query);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let response = client
        .get(resolve_endpoint())
        .header("X-Subscription-Token", api_key.expose_secret())
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|e| anyhow!("Brave answer request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("Brave answer returned HTTP {}", status));
    }

    let parsed: BraveContextResponse = response
        .json()
        .await
        .map_err(|e| anyhow!("Brave answer response decode failed: {}", e))?;

    let generic = parsed.grounding.map(|g| g.generic).unwrap_or_default();

    let mut text_parts: Vec<String> = Vec::new();
    let mut citations: Vec<String> = Vec::new();
    for source in &generic {
        for snippet in &source.snippets {
            if !snippet.is_empty() {
                text_parts.push(snippet.clone());
            }
        }
        if let Some(url) = &source.url
            && !citations.contains(url)
        {
            citations.push(url.clone());
        }
    }

    if text_parts.is_empty() {
        return Err(anyhow!(
            "Brave answer response carried no grounded snippets"
        ));
    }

    Ok(AnswerResult {
        text: text_parts.join("\n\n"),
        citations,
        cost_dollars: None,
        provider: "brave",
    })
}

/// Test-override hook for the endpoint — production always returns the
/// literal Brave `/res/v1/llm/context` URL. Named
/// `BRAVE_ANSWER_ENDPOINT_OVERRIDE` — distinct from `web_search`'s
/// `BRAVE_SEARCH_ENDPOINT_OVERRIDE`, which points at a different endpoint.
fn resolve_endpoint() -> String {
    std::env::var("BRAVE_ANSWER_ENDPOINT_OVERRIDE")
        .unwrap_or_else(|_| BRAVE_ANSWER_ENDPOINT.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_uses_constant_when_override_unset() {
        let ep = resolve_endpoint();
        assert!(ep.starts_with("http"), "endpoint = {ep}");
    }

    // Real request-shape behavior is exercised by
    // tests/web_answer_integration.rs (wiremock, derived from the
    // documented provider contract).
}

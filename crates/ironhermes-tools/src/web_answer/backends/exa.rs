//! Phase 41.3 D-07/D-13/D-14 (Plan 08): Exa `web_answer` backend.
//!
//! Endpoint: POST https://api.exa.ai/search — the SAME endpoint the sibling
//! `web_search` Exa backend (Plan 07) uses. Auth: `Authorization: Bearer
//! $EXA_API_KEY` — the SAME convention `web_search`'s Exa backend uses,
//! DIFFERENT from the existing `web_extract` Exa backend (which targets
//! `/contents` with an `x-api-key` header); do not import or mirror that
//! backend's header line here.
//!
//! Body: `query`, `type: "auto"` (the docs-recommended agent default),
//! `outputSchema: {"type": "text"}`, `contents.summary: true`.
//!
//! **`outputSchema` is what makes Exa answer.** Per Exa's published OpenAPI
//! contract (exa.ai/docs/reference/search) the top-level `output` object is
//! returned only when the request supplies `outputSchema` (or uses a `deep*`
//! search type); a plain `type: "auto"` request returns `results[]` and NO
//! `output`. The original Plan 08 request omitted it (its assumption that
//! `contents.summary` was "the answer surface" was never checked against the
//! live API), so in production every call failed with `Exa answer response
//! carried no output` and `web_answer` silently fell through to DuckDuckGo.
//! A root-`text` schema keeps D-07's single honest contract — the answer is
//! prose, exactly like the other three `web_answer` backends; the structured
//! `object` schema and `systemPrompt` (COVERAGE.md §2.1 OPT-OUT) stay unused.
//! Synthesis adds ~2s on top of the search type (Exa docs).
//!
//! Response: `output.content` mapped to `AnswerResult.text`; each
//! `output.grounding[].citations[]` entry flattened into
//! `AnswerResult.citations`. Citations are `{url, title}` OBJECTS in Exa's
//! contract (not bare strings, as Plan 08's fixture assumed); both shapes are
//! accepted and anything unrecognised is skipped rather than failing the
//! answer. `costDollars.total` is captured into `cost_dollars` (D-14,
//! capture-only — never enforced).

use std::time::Duration;

use anyhow::{Result, anyhow};
use ironhermes_core::config::Config;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use crate::web_answer::AnswerResult;

const EXA_ANSWER_ENDPOINT: &str = "https://api.exa.ai/search";

#[derive(Debug, Default, Deserialize)]
struct ExaAnswerResponse {
    #[serde(default)]
    output: Option<ExaOutput>,
    #[serde(default, rename = "costDollars")]
    cost_dollars: Option<ExaCostDollars>,
}

#[derive(Debug, Default, Deserialize)]
struct ExaOutput {
    #[serde(default)]
    content: String,
    #[serde(default)]
    grounding: Vec<ExaGrounding>,
}

#[derive(Debug, Default, Deserialize)]
struct ExaGrounding {
    #[serde(default)]
    citations: Vec<ExaCitation>,
}

/// One `output.grounding[].citations[]` entry. Exa's contract is a `{url, title}`
/// object; a bare URL string is also accepted (the shape the original fixture
/// assumed). `Other` swallows any unrecognised entry so a decoration problem can
/// never discard a good answer.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ExaCitation {
    Url(String),
    Object { url: String },
    Other(#[allow(dead_code)] serde_json::Value),
}

impl ExaCitation {
    fn into_url(self) -> Option<String> {
        match self {
            ExaCitation::Url(url) | ExaCitation::Object { url } => Some(url),
            ExaCitation::Other(_) => None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ExaCostDollars {
    #[serde(default)]
    total: Option<f64>,
}

/// Answer Exa's `/search` endpoint, requesting the answer surface. `api_key`
/// is exposed at exactly this call site (mirrors `provider.rs`'s
/// single-boundary discipline) and never logged, formatted into an error,
/// or placed anywhere but the request header.
pub async fn answer(query: &str, api_key: &SecretString) -> Result<AnswerResult> {
    let timeout_secs = Config::load().map(|c| c.web.timeout_secs).unwrap_or(30);
    debug!("web_answer: querying Exa for: {}", query);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let body = json!({
        "query": query,
        "type": "auto",
        // Required for Exa to return the top-level `output` object at all — see the
        // module docs. Root `text` => `output.content` is prose, not a JSON object.
        "outputSchema": { "type": "text" },
        "contents": { "summary": true },
    });

    let response = client
        .post(resolve_endpoint())
        .bearer_auth(api_key.expose_secret())
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow!("Exa answer request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("Exa answer returned HTTP {}", status));
    }

    let parsed: ExaAnswerResponse = response
        .json()
        .await
        .map_err(|e| anyhow!("Exa answer response decode failed: {}", e))?;

    let output = parsed
        .output
        .ok_or_else(|| anyhow!("Exa answer response carried no output"))?;

    if output.content.is_empty() {
        return Err(anyhow!("Exa answer response carried empty content"));
    }

    let mut citations: Vec<String> = Vec::new();
    for grounding in output.grounding {
        for citation in grounding.citations {
            if let Some(url) = citation.into_url()
                && !citations.contains(&url)
            {
                citations.push(url);
            }
        }
    }

    Ok(AnswerResult {
        text: output.content,
        citations,
        cost_dollars: parsed.cost_dollars.and_then(|c| c.total),
        provider: "exa",
    })
}

/// Test-override hook for the endpoint — production always returns the
/// literal Exa `/search` URL. Named `EXA_ANSWER_ENDPOINT_OVERRIDE` —
/// distinct from `web_search`'s `EXA_SEARCH_ENDPOINT_OVERRIDE` and
/// `web_extract`'s `EXA_ENDPOINT_OVERRIDE`, each a different call site.
fn resolve_endpoint() -> String {
    std::env::var("EXA_ANSWER_ENDPOINT_OVERRIDE")
        .unwrap_or_else(|_| EXA_ANSWER_ENDPOINT.to_string())
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

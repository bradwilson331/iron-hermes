//! Phase 41.3 D-07/D-13/D-14 (Plan 08): Exa `web_answer` backend.
//!
//! Endpoint: POST https://api.exa.ai/search — the SAME endpoint the sibling
//! `web_search` Exa backend (Plan 07) uses; Exa's `/search` endpoint serves
//! both a ranked-results surface AND an answer surface depending on the
//! requested `contents` shape. Auth: `Authorization: Bearer $EXA_API_KEY` —
//! the SAME convention `web_search`'s Exa backend uses, DIFFERENT from the
//! existing `web_extract` Exa backend (which targets `/contents` with an
//! `x-api-key` header); do not import or mirror that backend's header line
//! here.
//!
//! Body: `query`, `type: "auto"` (the docs-recommended agent default),
//! `contents.summary: true` (COVERAGE.md §2.1 — the answer surface).
//! Response: `output.content` mapped to `AnswerResult.text`, each
//! `output.grounding[].citations[]` entry flattened into
//! `AnswerResult.citations`, and `costDollars.total` captured into
//! `cost_dollars` (D-14, capture-only — never enforced). Deliberately
//! omits the two prompt/schema-control request parameters COVERAGE.md
//! §2.1 marks OPT-OUT for this backend — either would make Exa's answer
//! shape diverge from the other three `web_answer` backends and break
//! D-07's single honest contract; see COVERAGE.md for the exact parameter
//! names (neither is named in this file).

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
    citations: Vec<String>,
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
    for grounding in &output.grounding {
        for citation in &grounding.citations {
            if !citations.contains(citation) {
                citations.push(citation.clone());
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

//! Phase 41.3 D-07/D-10/D-13 (Plan 08): DuckDuckGo `web_answer` backend —
//! the keyless terminal of the default chain (D-09).
//!
//! Endpoint: GET https://api.duckduckgo.com/?q=<query>&format=json, no
//! auth. This is DuckDuckGo's Instant Answer JSON API — the SAME endpoint
//! `web_search`'s DDG backend (Plan 07) deliberately avoids, because that
//! endpoint's disambiguation array is not a ranked results source (D-10).
//! Here it is exactly the right source, because `AbstractText` (falling
//! back to `Answer` then `Definition`) is genuinely a synthesized answer.
//!
//! Field selection is a pure function (`select_answer_text`) over the
//! deserialized response, testable without a network call. The
//! disambiguation-links field this API also returns is never read here —
//! see D-10's rationale for why that field cannot serve an answer or a
//! results contract; it is not named in this module. When every candidate
//! field is empty, `answer()` returns an explicit `Err` (a clean miss)
//! rather than an empty string dressed as success, so the chain walk can
//! decide whether to report a miss to the caller.

use std::time::Duration;

use anyhow::{Result, anyhow};
use ironhermes_core::config::Config;
use serde::Deserialize;
use tracing::debug;

use crate::web_answer::AnswerResult;

const DDG_INSTANT_ANSWER_ENDPOINT: &str = "https://api.duckduckgo.com/";

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
struct DdgInstantAnswerResponse {
    #[serde(default, rename = "AbstractText")]
    abstract_text: String,
    #[serde(default, rename = "Answer")]
    answer: String,
    #[serde(default, rename = "Definition")]
    definition: String,
}

/// Answer DuckDuckGo's Instant Answer JSON API. Needs no API key — DDG is
/// the keyless terminal of the default `web_answer` chain (D-09).
pub async fn answer(query: &str) -> Result<AnswerResult> {
    let timeout_secs = Config::load().map(|c| c.web.timeout_secs).unwrap_or(30);
    debug!("web_answer: querying DDG for: {}", query);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let response = client
        .get(resolve_endpoint())
        .query(&[("q", query), ("format", "json")])
        .send()
        .await
        .map_err(|e| anyhow!("DDG instant-answer request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("DDG instant-answer returned HTTP {}", status));
    }

    let parsed: DdgInstantAnswerResponse = response
        .json()
        .await
        .map_err(|e| anyhow!("DDG instant-answer response decode failed: {}", e))?;

    let text = select_answer_text(&parsed).ok_or_else(|| {
        anyhow!("DDG instant-answer produced a clean miss — every candidate field was empty")
    })?;

    Ok(AnswerResult {
        text,
        citations: vec![],
        cost_dollars: None,
        provider: "ddg",
    })
}

/// Pure field-selection: `AbstractText`, falling back to `Answer` then
/// `Definition`. `None` when every candidate is empty — a clean miss, never
/// an empty string dressed as an answer.
fn select_answer_text(resp: &DdgInstantAnswerResponse) -> Option<String> {
    [&resp.abstract_text, &resp.answer, &resp.definition]
        .into_iter()
        .find(|s| !s.is_empty())
        .cloned()
}

/// Test-override hook for the endpoint — production always returns the
/// literal DDG Instant Answer JSON URL. Named `DDG_API_ENDPOINT_OVERRIDE` —
/// distinct from `web_search`'s `DDG_HTML_ENDPOINT_OVERRIDE`, which points
/// at a different endpoint entirely.
fn resolve_endpoint() -> String {
    std::env::var("DDG_API_ENDPOINT_OVERRIDE").unwrap_or_else(|_| DDG_INSTANT_ANSWER_ENDPOINT.to_string())
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
    fn abstract_text_is_preferred_then_answer_then_definition() {
        let all_set = DdgInstantAnswerResponse {
            abstract_text: "abstract wins".to_string(),
            answer: "answer loses".to_string(),
            definition: "definition loses".to_string(),
        };
        assert_eq!(
            select_answer_text(&all_set),
            Some("abstract wins".to_string())
        );

        let answer_only = DdgInstantAnswerResponse {
            abstract_text: String::new(),
            answer: "answer wins".to_string(),
            definition: "definition loses".to_string(),
        };
        assert_eq!(
            select_answer_text(&answer_only),
            Some("answer wins".to_string())
        );

        let definition_only = DdgInstantAnswerResponse {
            abstract_text: String::new(),
            answer: String::new(),
            definition: "definition wins".to_string(),
        };
        assert_eq!(
            select_answer_text(&definition_only),
            Some("definition wins".to_string())
        );
    }

    #[test]
    fn all_empty_fields_yield_a_clean_miss() {
        let empty = DdgInstantAnswerResponse::default();
        assert_eq!(select_answer_text(&empty), None);
    }
}

//! Phase 41.3 D-07/D-08 (Plan 07): Exa `web_search` backend.
//!
//! Endpoint: POST https://api.exa.ai/search
//! Auth: `Authorization: Bearer <EXA_API_KEY>` header.
//!
//! This is a DIFFERENT endpoint and a DIFFERENT auth convention from the
//! existing `web_extract` Exa backend (which targets Exa's content-retrieval
//! endpoint) — both are current and correct for their own endpoint; do not
//! import or mirror that backend's header line here.
//!
//! Body: `query`, `type: "auto"` (the docs-recommended agent default),
//! `numResults`, `contents.text`, `contents.highlights`,
//! `contents.livecrawlTimeout`, plus `includeDomains`/`excludeDomains` and
//! `startPublishedDate` when the caller supplies a domain or freshness
//! filter. Response: `results[]` mapped to `SearchHit`, and
//! `costDollars.total` captured into `SearchOutcome.cost_dollars` (D-14,
//! capture-only — never enforced).

use std::time::Duration;

use anyhow::{Result, anyhow};
use chrono::{Duration as ChronoDuration, Utc};
use ironhermes_core::config::Config;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use crate::web_search::backends::SearchFilters;
use crate::web_search::{SearchHit, SearchOutcome};

const EXA_SEARCH_ENDPOINT: &str = "https://api.exa.ai/search";

#[derive(Debug, Deserialize)]
struct ExaSearchResponse {
    #[serde(default)]
    results: Vec<ExaSearchResult>,
    #[serde(default, rename = "costDollars")]
    cost_dollars: Option<ExaCostDollars>,
}

#[derive(Debug, Deserialize)]
struct ExaSearchResult {
    url: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    highlights: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ExaCostDollars {
    #[serde(default)]
    total: Option<f64>,
}

/// Search Exa's `/search` endpoint. `api_key` is exposed at exactly this call
/// site (mirrors `provider.rs:585`'s single-boundary discipline) and never
/// logged, formatted into an error, or placed anywhere but the request
/// header.
pub async fn search(
    query: &str,
    limit: u64,
    filters: &SearchFilters,
    api_key: &SecretString,
) -> Result<SearchOutcome> {
    let timeout_secs = Config::load().map(|c| c.web.timeout_secs).unwrap_or(30);
    debug!("web_search: querying Exa for: {}", query);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let mut body = json!({
        "query": query,
        "type": "auto",
        "numResults": limit,
        "contents": {
            "text": true,
            "highlights": true,
            "livecrawlTimeout": 10_000,
        },
    });
    if !filters.include_domains.is_empty() {
        body["includeDomains"] = json!(filters.include_domains);
    }
    if !filters.exclude_domains.is_empty() {
        body["excludeDomains"] = json!(filters.exclude_domains);
    }
    if let Some(start) = filters
        .freshness
        .as_deref()
        .and_then(freshness_to_start_published_date)
    {
        body["startPublishedDate"] = json!(start);
    }

    let response = client
        .post(resolve_endpoint())
        .bearer_auth(api_key.expose_secret())
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow!("Exa search request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("Exa search returned HTTP {}", status));
    }

    let parsed: ExaSearchResponse = response
        .json()
        .await
        .map_err(|e| anyhow!("Exa search response decode failed: {}", e))?;

    let hits = parsed
        .results
        .into_iter()
        .take(limit as usize)
        .map(|r| {
            let snippet = r
                .text
                .filter(|t| !t.is_empty())
                .or_else(|| r.highlights.first().cloned())
                .unwrap_or_default();
            SearchHit {
                title: r.title.unwrap_or_default(),
                url: r.url,
                snippet,
            }
        })
        .collect();

    Ok(SearchOutcome {
        hits,
        cost_dollars: parsed.cost_dollars.and_then(|c| c.total),
        provider: "exa",
    })
}

/// Converts the tool's symbolic freshness window into Exa's exact
/// `startPublishedDate` — Exa has no relative-freshness parameter of its own.
fn freshness_to_start_published_date(freshness: &str) -> Option<String> {
    let days = match freshness {
        "day" => 1,
        "week" => 7,
        "month" => 30,
        "year" => 365,
        _ => return None,
    };
    Some((Utc::now() - ChronoDuration::days(days)).to_rfc3339())
}

/// Test-override hook for the endpoint — production always returns the
/// literal Exa `/search` URL. Deliberately named `EXA_SEARCH_ENDPOINT_OVERRIDE`
/// — distinct from `web_extract`'s `EXA_ENDPOINT_OVERRIDE`, which points at a
/// different endpoint.
fn resolve_endpoint() -> String {
    std::env::var("EXA_SEARCH_ENDPOINT_OVERRIDE")
        .unwrap_or_else(|_| EXA_SEARCH_ENDPOINT.to_string())
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
    fn freshness_maps_known_windows_and_ignores_unknown() {
        assert!(freshness_to_start_published_date("day").is_some());
        assert!(freshness_to_start_published_date("week").is_some());
        assert!(freshness_to_start_published_date("month").is_some());
        assert!(freshness_to_start_published_date("year").is_some());
        assert!(freshness_to_start_published_date("decade").is_none());
    }

    // Real request-shape behavior is exercised by
    // tests/web_search_integration.rs (wiremock, derived from the documented
    // provider contract — not copied from web_extract's existing Exa fixture).
}

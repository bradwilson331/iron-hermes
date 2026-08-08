//! Phase 41.3 D-07/D-08 (Plan 07): Tavily `web_search` backend.
//!
//! Endpoint: POST https://api.tavily.com/search
//! Auth: `Authorization: Bearer <TAVILY_API_KEY>` header.
//!
//! Body: `query`, `max_results`, `search_depth: "basic"` (pinned per
//! COVERAGE.md §2.2), `topic: "general"` (pinned), `chunks_per_source`,
//! `include_usage: true` (D-14), plus `include_domains`/`exclude_domains`
//! and `time_range` when the caller supplies a domain or freshness filter.
//! `include_answer` is deliberately left off — that belongs to `web_answer`
//! (Plan 08), not this results-list tool (D-07).
//!
//! Response: `results[]` (`title`/`url`/`content`) mapped to `SearchHit`,
//! and `usage.credits` recorded into `SearchOutcome.cost_dollars` (D-14,
//! capture-only). Tavily's usage figure is a credit count, not literally
//! dollars, but it fills the same "record the provider's usage figure"
//! contract Exa's `costDollars.total` does.

use std::time::Duration;

use anyhow::{Result, anyhow};
use ironhermes_core::config::Config;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use crate::web_search::backends::SearchFilters;
use crate::web_search::{SearchHit, SearchOutcome};

const TAVILY_SEARCH_ENDPOINT: &str = "https://api.tavily.com/search";

#[derive(Debug, Deserialize)]
struct TavilySearchResponse {
    #[serde(default)]
    results: Vec<TavilySearchResult>,
    #[serde(default)]
    usage: Option<TavilyUsage>,
}

#[derive(Debug, Deserialize)]
struct TavilySearchResult {
    #[serde(default)]
    title: Option<String>,
    url: String,
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TavilyUsage {
    #[serde(default)]
    credits: Option<f64>,
}

/// Search Tavily's `/search` endpoint. `api_key` is exposed at exactly this
/// call site and never logged, formatted into an error, or placed anywhere
/// but the request header.
pub async fn search(
    query: &str,
    limit: u64,
    filters: &SearchFilters,
    api_key: &SecretString,
) -> Result<SearchOutcome> {
    let timeout_secs = Config::load().map(|c| c.web.timeout_secs).unwrap_or(30);
    debug!("web_search: querying Tavily for: {}", query);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let mut body = json!({
        "query": query,
        "max_results": limit,
        "search_depth": "basic",
        "topic": "general",
        "chunks_per_source": 3,
        "include_usage": true,
    });
    if !filters.include_domains.is_empty() {
        body["include_domains"] = json!(filters.include_domains);
    }
    if !filters.exclude_domains.is_empty() {
        body["exclude_domains"] = json!(filters.exclude_domains);
    }
    if let Some(freshness) = filters.freshness.as_deref()
        && matches!(freshness, "day" | "week" | "month" | "year")
    {
        body["time_range"] = json!(freshness);
    }

    let response = client
        .post(resolve_endpoint())
        .bearer_auth(api_key.expose_secret())
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow!("Tavily search request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("Tavily search returned HTTP {}", status));
    }

    let parsed: TavilySearchResponse = response
        .json()
        .await
        .map_err(|e| anyhow!("Tavily search response decode failed: {}", e))?;

    let hits = parsed
        .results
        .into_iter()
        .take(limit as usize)
        .map(|r| SearchHit {
            title: r.title.unwrap_or_default(),
            url: r.url,
            snippet: r.content.unwrap_or_default(),
        })
        .collect();

    Ok(SearchOutcome {
        hits,
        cost_dollars: parsed.usage.and_then(|u| u.credits),
        provider: "tavily",
    })
}

/// Test-override hook for the endpoint — production always returns the
/// literal Tavily `/search` URL. Deliberately named
/// `TAVILY_SEARCH_ENDPOINT_OVERRIDE` — distinct from `web_extract`'s
/// `TAVILY_ENDPOINT_OVERRIDE`, which points at a different endpoint.
fn resolve_endpoint() -> String {
    std::env::var("TAVILY_SEARCH_ENDPOINT_OVERRIDE")
        .unwrap_or_else(|_| TAVILY_SEARCH_ENDPOINT.to_string())
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
    // tests/web_search_integration.rs (wiremock, derived from the documented
    // provider contract).
}

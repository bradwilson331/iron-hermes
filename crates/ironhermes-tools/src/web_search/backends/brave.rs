//! Phase 41.3 D-07/D-08 (Plan 07): Brave `web_search` backend.
//!
//! Endpoints (routed by `SearchFilters::is_news()`, quick 261015-tcs
//! COVERAGE.md):
//! - default: GET https://api.search.brave.com/res/v1/web/search
//! - news category: GET https://api.search.brave.com/res/v1/news/search
//!
//! Auth: `X-Subscription-Token` header carrying BRAVE_API_KEY (both
//! endpoints) — a different convention from the other providers in this
//! chain; the backend trait deliberately assumes no uniform auth scheme
//! across providers.
//!
//! Params: `q`, `count` (clamped to the endpoint's documented maximum: 20 for
//! web, 50 for news), `extra_snippets`, plus `freshness` when the caller
//! supplies a freshness filter. Brave has no domain include/exclude parameter
//! on these endpoints (COVERAGE.md §2.3 — provider limitation), so that
//! filter is silently ignored here.
//!
//! Response: web `web.results[]`, or news top-level `results[]`
//! (`title`/`url`/`description` in both), mapped to `SearchHit`. Brave
//! reports no cost field, so `cost_dollars` is always `None`.

use std::time::Duration;

use anyhow::{Result, anyhow};
use ironhermes_core::config::Config;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tracing::debug;

use crate::web_search::backends::SearchFilters;
use crate::web_search::{SearchHit, SearchOutcome};

const BRAVE_SEARCH_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
/// Brave's documented maximum for `count` on `/res/v1/web/search`.
const BRAVE_MAX_COUNT: u64 = 20;
const BRAVE_NEWS_ENDPOINT: &str = "https://api.search.brave.com/res/v1/news/search";
/// Brave's documented maximum for `count` on `/res/v1/news/search`.
const BRAVE_NEWS_MAX_COUNT: u64 = 50;

/// News endpoint response: `results[]` sits at the top level, not under `web`.
#[derive(Debug, Deserialize)]
struct BraveNewsResponse {
    #[serde(default)]
    results: Vec<BraveResult>,
}

#[derive(Debug, Deserialize)]
struct BraveSearchResponse {
    #[serde(default)]
    web: Option<BraveWebResults>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResults {
    #[serde(default)]
    results: Vec<BraveResult>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    #[serde(default)]
    title: Option<String>,
    url: String,
    #[serde(default)]
    description: Option<String>,
}

/// Search Brave's `/res/v1/web/search` endpoint. `api_key` is exposed at
/// exactly this call site and never logged, formatted into an error, or
/// placed anywhere but the request header.
pub async fn search(
    query: &str,
    limit: u64,
    filters: &SearchFilters,
    api_key: &SecretString,
) -> Result<SearchOutcome> {
    let timeout_secs = Config::load().map(|c| c.web.timeout_secs).unwrap_or(30);
    debug!("web_search: querying Brave for: {}", query);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let news = filters.is_news();
    let (endpoint, max_count, label) = if news {
        (
            resolve_news_endpoint(),
            BRAVE_NEWS_MAX_COUNT,
            "Brave news search",
        )
    } else {
        (resolve_endpoint(), BRAVE_MAX_COUNT, "Brave search")
    };
    let count = limit.clamp(1, max_count);
    let mut query_params: Vec<(&str, String)> = vec![
        ("q", query.to_string()),
        ("count", count.to_string()),
        ("extra_snippets", "true".to_string()),
    ];
    if let Some(freshness) = filters
        .freshness
        .as_deref()
        .and_then(freshness_to_brave_param)
    {
        query_params.push(("freshness", freshness.to_string()));
    }

    let response = client
        .get(endpoint)
        .header("X-Subscription-Token", api_key.expose_secret())
        .query(&query_params)
        .send()
        .await
        .map_err(|e| anyhow!("{} request failed: {}", label, e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("{} returned HTTP {}", label, status));
    }

    let results = if news {
        let parsed: BraveNewsResponse = response
            .json()
            .await
            .map_err(|e| anyhow!("{} response decode failed: {}", label, e))?;
        parsed.results
    } else {
        let parsed: BraveSearchResponse = response
            .json()
            .await
            .map_err(|e| anyhow!("{} response decode failed: {}", label, e))?;
        parsed.web.map(|w| w.results).unwrap_or_default()
    };

    let hits = results
        .into_iter()
        .take(limit as usize)
        .map(|r| SearchHit {
            title: r.title.unwrap_or_default(),
            url: r.url,
            snippet: r.description.unwrap_or_default(),
        })
        .collect();

    Ok(SearchOutcome {
        hits,
        cost_dollars: None,
        provider: "brave",
    })
}

/// Converts the tool's symbolic freshness window into Brave's documented
/// `freshness` values (past day/week/month/year).
fn freshness_to_brave_param(freshness: &str) -> Option<&'static str> {
    match freshness {
        "day" => Some("pd"),
        "week" => Some("pw"),
        "month" => Some("pm"),
        "year" => Some("py"),
        _ => None,
    }
}

/// Test-override hook for the endpoint — production always returns the
/// literal Brave `/web/search` URL.
fn resolve_endpoint() -> String {
    std::env::var("BRAVE_SEARCH_ENDPOINT_OVERRIDE")
        .unwrap_or_else(|_| BRAVE_SEARCH_ENDPOINT.to_string())
}

/// Test-override hook for the news endpoint. Deliberately distinct from
/// `BRAVE_SEARCH_ENDPOINT_OVERRIDE`: each endpoint override names exactly one
/// endpoint, matching this crate's convention.
fn resolve_news_endpoint() -> String {
    std::env::var("BRAVE_NEWS_ENDPOINT_OVERRIDE")
        .unwrap_or_else(|_| BRAVE_NEWS_ENDPOINT.to_string())
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
    fn count_clamp_caps_at_the_documented_maximum() {
        assert_eq!(25u64.clamp(1, BRAVE_MAX_COUNT), BRAVE_MAX_COUNT);
        assert_eq!(5u64.clamp(1, BRAVE_MAX_COUNT), 5);
    }

    #[test]
    fn news_endpoint_uses_constant_when_override_unset() {
        let ep = resolve_news_endpoint();
        assert!(ep.starts_with("http"), "endpoint = {ep}");
    }

    #[test]
    fn news_count_clamp_caps_at_fifty() {
        assert_eq!(60u64.clamp(1, BRAVE_NEWS_MAX_COUNT), 50);
        assert_eq!(BRAVE_NEWS_MAX_COUNT, 50);
    }

    // Real request-shape behavior is exercised by
    // tests/web_search_integration.rs (wiremock, derived from the documented
    // provider contract).
}

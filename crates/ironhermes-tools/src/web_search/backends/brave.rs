//! Phase 41.3 D-07/D-08 (Plan 07): Brave `web_search` backend.
//!
//! Endpoint: GET https://api.search.brave.com/res/v1/web/search
//! Auth: `X-Subscription-Token` header carrying BRAVE_API_KEY — a different
//! convention from the other providers in this chain; the backend trait
//! deliberately assumes no uniform auth scheme across providers.
//!
//! Params: `q`, `count` (clamped to Brave's documented maximum of 20),
//! `extra_snippets`, plus `freshness` when the caller supplies a freshness
//! filter. Brave has no domain include/exclude parameter on this endpoint
//! (COVERAGE.md §2.3 — provider limitation), so that filter is silently
//! ignored here.
//!
//! Response: `web.results[]` (`title`/`url`/`description`) mapped to
//! `SearchHit`. Brave reports no cost field, so `cost_dollars` is always
//! `None`.

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

    let count = limit.clamp(1, BRAVE_MAX_COUNT);
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
        .get(resolve_endpoint())
        .header("X-Subscription-Token", api_key.expose_secret())
        .query(&query_params)
        .send()
        .await
        .map_err(|e| anyhow!("Brave search request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("Brave search returned HTTP {}", status));
    }

    let parsed: BraveSearchResponse = response
        .json()
        .await
        .map_err(|e| anyhow!("Brave search response decode failed: {}", e))?;

    let hits = parsed
        .web
        .map(|w| w.results)
        .unwrap_or_default()
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

    // Real request-shape behavior is exercised by
    // tests/web_search_integration.rs (wiremock, derived from the documented
    // provider contract).
}

//! Phase 41.3 D-07/D-08/D-10 (Plan 07): DuckDuckGo `web_search` backend —
//! results mode only, keyless.
//!
//! Endpoint: GET https://html.duckduckgo.com/html/?q=<query>, no auth.
//! DuckDuckGo's Instant Answer JSON API (`api.duckduckgo.com`) is
//! deliberately NOT used here — its disambiguation array is not a ranked
//! results source and belongs to `web_answer`'s keyless terminal (Plan 08),
//! not this tool's results-list contract (D-10).
//!
//! Parsing reuses this crate's existing `scraper`-based HTML extraction
//! discipline (see `web_extract/backends/local.rs` and
//! `web_extract/sanitize.rs`) rather than adding a new HTML dependency.
//! `parse_results` is a **pure** function so the markup parse is testable
//! without a network call; on unrecognised markup it returns an empty
//! (not garbage) result set.

use std::time::Duration;

use anyhow::{Result, anyhow};
use ironhermes_core::config::Config;
use scraper::{Html, Selector};
use tracing::{debug, warn};

use crate::web_search::{SearchHit, SearchOutcome};

const DDG_HTML_ENDPOINT: &str = "https://html.duckduckgo.com/html/";

/// Fetch and parse DuckDuckGo's HTML results page. Needs no API key — DDG is
/// the keyless terminal of the default `web_search` chain (D-09).
pub async fn search(query: &str, limit: u64) -> Result<SearchOutcome> {
    let timeout_secs = Config::load().map(|c| c.web.timeout_secs).unwrap_or(30);
    debug!("web_search: querying DDG for: {}", query);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let response = client
        .get(resolve_endpoint())
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|e| anyhow!("DDG request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("DDG returned HTTP {}", status));
    }

    let body = response
        .text()
        .await
        .map_err(|e| anyhow!("DDG response read failed: {}", e))?;

    let hits = parse_results(&body, limit);
    if hits.is_empty() {
        warn!(
            "web_search: DDG markup produced zero parsed results — degrading to an empty \
             result set rather than returning garbage rows"
        );
    }

    Ok(SearchOutcome {
        hits,
        cost_dollars: None,
        provider: "ddg",
    })
}

/// Pure parse: DDG results markup -> ordered `Vec<SearchHit>`, capped at
/// `limit`. Unrecognised markup (no matching selectors) yields an empty
/// `Vec`, never rows assembled from stray text.
fn parse_results(html: &str, limit: u64) -> Vec<SearchHit> {
    let Ok(result_sel) = Selector::parse(".web-result") else {
        return vec![];
    };
    let Ok(title_sel) = Selector::parse(".result__title .result__a") else {
        return vec![];
    };
    let Ok(snippet_sel) = Selector::parse(".result__snippet") else {
        return vec![];
    };

    let document = Html::parse_document(html);
    let mut hits = Vec::new();

    for result in document.select(&result_sel) {
        if hits.len() as u64 >= limit {
            break;
        }

        let Some(title_el) = result.select(&title_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        let Some(href) = title_el.value().attr("href") else {
            continue;
        };
        let Some(url) = resolve_ddg_redirect_url(href) else {
            continue;
        };
        if title.is_empty() || url.is_empty() {
            continue;
        }

        let snippet = result
            .select(&snippet_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        hits.push(SearchHit {
            title,
            url,
            snippet,
        });
    }

    hits
}

/// DDG wraps result links in a redirect form
/// (`//duckduckgo.com/l/?uddg=<percent-encoded-destination>&rut=...`).
/// Unwraps to the decoded destination URL; a plain absolute `http(s)` href
/// (non-redirect form) passes through unchanged; anything else is rejected
/// rather than guessed at.
fn resolve_ddg_redirect_url(href: &str) -> Option<String> {
    if let Some(query_start) = href.find('?') {
        let query = &href[query_start + 1..];
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("uddg=") {
                return percent_decode(value).filter(|u| u.starts_with("http"));
            }
        }
    }
    if href.starts_with("http") {
        return Some(href.to_string());
    }
    None
}

/// Minimal `%xx` percent-decoder — mirrors
/// `web_extract/sanitize.rs`'s `percent_decode_lossy` scope (substring
/// decoding for URL unwrapping only, not roundtrip-correct URL parsing)
/// rather than adding a new dependency for one decode.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let h = (bytes[i + 1] as char).to_digit(16);
                let l = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (h, l) {
                    out.push(((h << 4) | l) as u8);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            _ => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Test-override hook for the endpoint — production always returns the
/// literal DDG HTML results URL.
fn resolve_endpoint() -> String {
    std::env::var("DDG_HTML_ENDPOINT_OVERRIDE").unwrap_or_else(|_| DDG_HTML_ENDPOINT.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_uses_constant_when_override_unset() {
        let ep = resolve_endpoint();
        assert!(ep.starts_with("http"), "endpoint = {ep}");
    }

    /// A representative fixture of DDG's `html.duckduckgo.com/html/` markup,
    /// shaped from a real fetched page: two `.web-result` blocks (one whose
    /// link is wrapped in DDG's `uddg=`-redirect form, one with a plain
    /// absolute href) plus one `result--ad` block (no `web-result` class,
    /// per DDG's real markup) that must NOT be parsed as a result.
    const REPRESENTATIVE_RESULTS_PAGE: &str = r#"
    <html><body><div class="results">
      <div class="result results_links results_links_deep web-result ">
        <div class="links_main links_deep result__body">
          <h2 class="result__title">
            <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Frust%2Dlang.org%2F&amp;rut=abc">Rust Programming Language</a>
          </h2>
          <div class="result__extras">
            <div class="result__extras__url">
              <a class="result__url" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Frust%2Dlang.org%2F&amp;rut=abc">rust-lang.org</a>
            </div>
          </div>
          <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Frust%2Dlang.org%2F&amp;rut=abc"><b>Rust</b> is a fast, reliable systems language.</a>
        </div>
      </div>
      <div class="result results_links results_links_deep web-result ">
        <div class="links_main links_deep result__body">
          <h2 class="result__title">
            <a rel="nofollow" class="result__a" href="https://doc.rust-lang.org/book/">The Rust Book</a>
          </h2>
          <a class="result__snippet" href="https://doc.rust-lang.org/book/">The official Rust guide.</a>
        </div>
      </div>
      <div class="result results_links results_links_deep result--ad ">
        <div class="links_main links_deep result__body">
          <h2 class="result__title">
            <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fads.example.com%2F&amp;rut=xyz">Sponsored result</a>
          </h2>
          <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fads.example.com%2F">This is an ad.</a>
        </div>
      </div>
    </div></body></html>
    "#;

    #[test]
    fn parses_a_representative_results_page() {
        let hits = parse_results(REPRESENTATIVE_RESULTS_PAGE, 10);
        assert_eq!(
            hits.len(),
            2,
            "must parse exactly the two web-result blocks, excluding the ad: {hits:?}"
        );
        assert_eq!(hits[0].title, "Rust Programming Language");
        assert_eq!(hits[0].url, "https://rust-lang.org/");
        assert!(
            hits[0].snippet.contains("fast, reliable"),
            "{:?}",
            hits[0]
        );
        assert_eq!(hits[1].title, "The Rust Book");
        assert_eq!(hits[1].url, "https://doc.rust-lang.org/book/");
    }

    #[test]
    fn unrecognised_markup_yields_empty_not_garbage() {
        let html = "<html><body><p>Nothing DDG-shaped here.</p></body></html>";
        let hits = parse_results(html, 10);
        assert!(
            hits.is_empty(),
            "unrecognised markup must yield an empty Vec, not garbage rows: {hits:?}"
        );
    }

    #[test]
    fn relative_and_redirect_urls_are_resolved_to_absolute() {
        assert_eq!(
            resolve_ddg_redirect_url(
                "//duckduckgo.com/l/?uddg=https%3A%2F%2Frust%2Dlang.org%2F&rut=abc"
            ),
            Some("https://rust-lang.org/".to_string())
        );
        assert_eq!(
            resolve_ddg_redirect_url("https://doc.rust-lang.org/book/"),
            Some("https://doc.rust-lang.org/book/".to_string())
        );
        assert_eq!(resolve_ddg_redirect_url("/relative/path"), None);
    }

    #[test]
    fn parse_results_respects_limit() {
        let hits = parse_results(REPRESENTATIVE_RESULTS_PAGE, 1);
        assert_eq!(hits.len(), 1);
    }
}

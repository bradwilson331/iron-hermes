//! Phase 41.3 D-07/D-08/D-09/D-10/D-14/D-19 (Plan 07): `web_search` —
//! config-ordered, multi-provider (Exa > Brave > Tavily > DuckDuckGo by
//! default), keeping exactly the pre-existing results-list contract
//! (`title` / `url` / `snippet`) and gaining no answer-synthesis switch —
//! that split is D-07's whole point: `web_answer` (Plan 08) is a separate
//! tool over the same providers, so the caller picks the tool by intent
//! instead of a capability-dependent parameter.

pub mod backends;

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tracing::debug;

use crate::credentials::ToolCredentials;
use crate::registry::Tool;

/// Phase 41.3 D-07 (Plan 07): one search result row — the tool's public
/// output contract. Every backend returns this exact shape regardless of
/// provider.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Phase 41.3 D-07/D-08/D-14 (Plan 07): one backend's search outcome — the
/// hits it returned, its optional cost/usage figure (D-14, capture-only),
/// and which provider answered. `provider` is not shown to the LLM; it is
/// used by `WebSearchTool`'s D-14 cost-capture log and by `search_chain`'s
/// own tests.
#[derive(Debug, Clone)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    pub cost_dollars: Option<f64>,
    pub provider: &'static str,
}

/// Phase 41.3 D-09 (Plan 07): any-of group id shared by every KEYED
/// `web_search` provider's prerequisite (Exa, Brave, Tavily) — DDG needs no
/// key and contributes none. `WebSearchTool::is_available()` does not
/// delegate to the trait-default group-aware walk that this group id would
/// otherwise drive (see that method's doc comment for why), but the group
/// still lets the setup wizard and `doctor` (Plan 09) render an N-of-M
/// "providers configured" count via `group_members`/`satisfied_group_members`
/// (registry.rs).
pub const WEB_SEARCH_PROVIDER_GROUP: &str = "web_search_provider";

/// Phase 41.3 Plan 11 (D-19): stateful — carries the resolved credential
/// snapshot so both `is_available()` and the chain walk read it instead of
/// reading the process environment directly. See `Default`'s doc comment
/// for the test/inspection-only escape hatch.
pub struct WebSearchTool {
    creds: Arc<ToolCredentials>,
}

impl WebSearchTool {
    /// Construct with a resolved credential snapshot — the production path.
    /// `register_defaults_except` and every delegate/RPC-sandbox construction
    /// site pass the registering `ToolRegistry`'s own snapshot here.
    pub fn new(creds: Arc<ToolCredentials>) -> Self {
        Self { creds }
    }

    /// Phase 41.3 D-08/D-19 (Plan 07): the config-ordered chain walk,
    /// returning the structured `SearchOutcome` — including which provider
    /// answered — rather than the formatted string `execute()` returns. A
    /// testable seam (mirrors this crate's `redact_url_args`/`process_one_url`
    /// convention): unit tests assert on `SearchOutcome.provider` directly
    /// instead of parsing rendered text.
    ///
    /// For each chain entry, asks `self.creds` (the D-19 snapshot) for that
    /// provider's key and skips the entry — before any network attempt —
    /// when the snapshot has none. A key held only in `tools.credentials` or
    /// only in the vault therefore counts as configured exactly like an env
    /// var would (D-19); this never reads a provider key directly from the
    /// process environment. On a backend error, warns and continues to the next entry;
    /// returns on the first success (regardless of whether that success has
    /// zero hits — a completed search with no results is still a completed
    /// search, matching the pre-Plan-07 single-provider contract). An
    /// unknown entry that slipped past `ToolsConfig::validate_chains()` is
    /// warned once and skipped, never a panic. On exhaustion, names every
    /// attempted provider and why it was skipped or failed, so an operator
    /// can act without enabling debug logging.
    async fn search_chain(
        &self,
        query: &str,
        limit: u64,
        filters: &backends::SearchFilters,
        chain: &[String],
    ) -> anyhow::Result<SearchOutcome> {
        let mut skip_or_fail_reasons: Vec<String> = Vec::new();

        for entry in chain {
            match entry.as_str() {
                "exa" => {
                    let Some(api_key) = self.creds.credential("EXA_API_KEY") else {
                        debug!("web_search: skipping exa in chain — EXA_API_KEY not configured");
                        skip_or_fail_reasons.push("exa: no key configured".to_string());
                        continue;
                    };
                    match backends::exa::search(query, limit, filters, &api_key).await {
                        Ok(outcome) => return Ok(outcome),
                        Err(e) => {
                            tracing::warn!("web_search: exa failed: {}", e);
                            skip_or_fail_reasons.push(format!("exa: {e}"));
                        }
                    }
                }
                "brave" => {
                    let Some(api_key) = self.creds.credential("BRAVE_API_KEY") else {
                        debug!(
                            "web_search: skipping brave in chain — BRAVE_API_KEY not configured"
                        );
                        skip_or_fail_reasons.push("brave: no key configured".to_string());
                        continue;
                    };
                    match backends::brave::search(query, limit, filters, &api_key).await {
                        Ok(outcome) => return Ok(outcome),
                        Err(e) => {
                            tracing::warn!("web_search: brave failed: {}", e);
                            skip_or_fail_reasons.push(format!("brave: {e}"));
                        }
                    }
                }
                "tavily" => {
                    let Some(api_key) = self.creds.credential("TAVILY_API_KEY") else {
                        debug!(
                            "web_search: skipping tavily in chain — TAVILY_API_KEY not configured"
                        );
                        skip_or_fail_reasons.push("tavily: no key configured".to_string());
                        continue;
                    };
                    match backends::tavily::search(query, limit, filters, &api_key).await {
                        Ok(outcome) => return Ok(outcome),
                        Err(e) => {
                            tracing::warn!("web_search: tavily failed: {}", e);
                            skip_or_fail_reasons.push(format!("tavily: {e}"));
                        }
                    }
                }
                "ddg" => match backends::ddg::search(query, limit).await {
                    Ok(outcome) => return Ok(outcome),
                    Err(e) => {
                        tracing::warn!("web_search: ddg failed: {}", e);
                        skip_or_fail_reasons.push(format!("ddg: {e}"));
                    }
                },
                unknown => {
                    tracing::warn!(
                        "web_search: unknown provider '{}' in tools.web_search.chain — skipping",
                        unknown
                    );
                    skip_or_fail_reasons
                        .push(format!("{unknown}: unrecognised provider, skipped"));
                }
            }
        }

        Err(anyhow::anyhow!(
            "web_search: no provider in the configured chain returned results ({})",
            if skip_or_fail_reasons.is_empty() {
                "chain is empty".to_string()
            } else {
                skip_or_fail_reasons.join("; ")
            }
        ))
    }
}

impl Default for WebSearchTool {
    /// Test/inspection-only: builds an env-only snapshot (`ToolCredentials::env_only()`).
    /// A PRODUCTION call site using this silently loses the config and vault
    /// credential tiers — see D-19's precedence chain in `credentials.rs`.
    /// `crates/ironhermes-cli/src/toolset_cmd.rs`'s inspection-only registries
    /// are the one documented exception (they list tool names and never
    /// execute anything).
    fn default() -> Self {
        Self::new(Arc::new(ToolCredentials::env_only()))
    }
}

/// Renders a `SearchOutcome` into the tool's byte-compatible output text —
/// unchanged from the pre-Plan-07 single-provider format: `"{N} result(s)
/// for '{query}':"` followed by numbered `{title}` / `URL: {url}` / snippet
/// blocks, snippet capped at 3 lines and 300 characters.
fn format_search_output(outcome: &SearchOutcome, query: &str) -> String {
    if outcome.hits.is_empty() {
        return format!("No results found for '{}'.", query);
    }

    let mut output = format!("{} result(s) for '{}':\n\n", outcome.hits.len(), query);

    for (i, hit) in outcome.hits.iter().enumerate() {
        let title = if hit.title.is_empty() {
            "(no title)"
        } else {
            hit.title.as_str()
        };
        let snippet = hit
            .snippet
            .lines()
            .take(3)
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(300)
            .collect::<String>();

        output.push_str(&format!(
            "{}. {}\n   URL: {}\n   {}\n\n",
            i + 1,
            title,
            hit.url,
            snippet
        ));
    }

    output.trim_end().to_string()
}

/// Extracts a `Vec<String>` from a JSON array argument, dropping non-string
/// entries rather than erroring — mirrors the tolerant-parsing convention
/// `WebExtractTool::execute` already uses for its own array arguments.
fn json_string_array(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn toolset(&self) -> &str {
        "web"
    }

    fn description(&self) -> &str {
        "Search the web and return relevant results (title, URL, snippet). Walks a \
         config-ordered chain of providers (Exa, Brave, Tavily, DuckDuckGo by default) and \
         is available even with no API key configured — DuckDuckGo needs none."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "web_search",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 10).",
                        "default": 10
                    },
                    "include_domains": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Restrict results to these domains, where the selected provider supports it."
                    },
                    "exclude_domains": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Exclude results from these domains, where the selected provider supports it."
                    },
                    "freshness": {
                        "type": "string",
                        "enum": ["day", "week", "month", "year"],
                        "description": "Restrict results to content published within this recency window, where the selected provider supports it."
                    }
                },
                "required": ["query"]
            }),
        )
    }

    /// D-09: `web_search` is available even with zero provider keys
    /// configured. The default chain always terminates in DDG, which needs
    /// no key, so a fresh install never hard-fails here. This intentionally
    /// does NOT delegate to the trait default's group-aware AND/OR walk —
    /// DDG contributes no prerequisite entry at all, so an empty-group edge
    /// case in that generic walk must not be the thing deciding availability
    /// for this tool.
    fn is_available(&self) -> bool {
        true
    }

    /// D-09: one grouped, non-required prerequisite per KEYED provider — the
    /// wizard prompts "pick a provider" and `doctor` (Plan 09) can render an
    /// N-of-M count. DDG requires nothing and contributes no entry.
    fn prerequisites(&self) -> Vec<crate::registry::Prerequisite> {
        vec![
            crate::registry::Prerequisite::grouped_env_var(
                "EXA_API_KEY",
                "Exa API key (https://dashboard.exa.ai) — one of several interchangeable \
                 web_search providers.",
                WEB_SEARCH_PROVIDER_GROUP,
            ),
            crate::registry::Prerequisite::grouped_env_var(
                "BRAVE_API_KEY",
                "Brave Search API key (https://api-dashboard.search.brave.com) — one of \
                 several interchangeable web_search providers.",
                WEB_SEARCH_PROVIDER_GROUP,
            ),
            crate::registry::Prerequisite::grouped_env_var(
                "TAVILY_API_KEY",
                "Tavily API key (https://app.tavily.com) — one of several interchangeable \
                 web_search providers.",
                WEB_SEARCH_PROVIDER_GROUP,
            ),
            // DDG needs no key and contributes no prerequisite — it is the
            // keyless terminal of the default chain (D-09).
        ]
    }

    fn credentials(&self) -> Option<&crate::credentials::ToolCredentials> {
        Some(self.creds.as_ref())
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: query"))?;
        let limit = args["limit"].as_u64().unwrap_or(10);
        let filters = backends::SearchFilters {
            include_domains: json_string_array(&args["include_domains"]),
            exclude_domains: json_string_array(&args["exclude_domains"]),
            freshness: args["freshness"].as_str().map(String::from),
        };

        // D-08: read the chain at call time — never cached at construction —
        // mirroring `fetch_web_with_chain`'s own convention (web_extract.rs).
        let cfg = ironhermes_core::config::Config::load().unwrap_or_default();
        let outcome = self
            .search_chain(query, limit, &filters, &cfg.tools.web_search.chain)
            .await?;

        // D-14: capture only, never enforced. One structured tracing event,
        // the same observability posture as Plan 01's timeout counter.
        if let Some(cost) = outcome.cost_dollars {
            tracing::info!(
                tool = "web_search",
                provider = outcome.provider,
                cost_dollars = cost,
                "web_search provider cost captured"
            );
        }

        Ok(format_search_output(&outcome, query))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;
    use tokio::sync::Mutex as TokioMutex;

    /// env_lock — serialise tests that mutate provider env vars. An
    /// async-aware `tokio::sync::Mutex`, not `std::sync::Mutex` — every test
    /// here is `#[tokio::test]` and some hold the guard across an `.await`
    /// (`ToolCredentials::resolve` / `tool.execute` / `search_chain`), and
    /// `clippy::await_holding_lock` (denied under this crate's `-D warnings`
    /// gate) forbids a std Mutex guard held across an await point.
    fn env_lock() -> &'static TokioMutex<()> {
        static LOCK: OnceLock<TokioMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| TokioMutex::new(()))
    }

    fn config_with_credential(key: &str, value: &str) -> ironhermes_core::Config {
        let mut config = ironhermes_core::Config::default();
        config
            .tools
            .credentials
            .insert(key.to_string(), value.to_string());
        config
    }

    fn clear_all_provider_keys() {
        // SAFETY: env_lock held, --test-threads=1 convention.
        unsafe {
            std::env::remove_var("EXA_API_KEY");
            std::env::remove_var("BRAVE_API_KEY");
            std::env::remove_var("TAVILY_API_KEY");
        }
    }

    /// D-09: `is_available()` must stay `true` with every provider key
    /// absent from the snapshot — DDG terminates the chain.
    #[tokio::test]
    async fn is_available_is_true_with_no_provider_keys_set() {
        let _g = env_lock().lock().await;
        clear_all_provider_keys();

        let creds = ToolCredentials::resolve(&ironhermes_core::Config::default(), None)
            .await
            .expect("resolve must succeed");
        let tool = WebSearchTool::new(Arc::new(creds));

        assert!(
            tool.is_available(),
            "web_search must be available with zero provider keys — DDG terminates the chain (D-09)"
        );
    }

    /// D-09: every returned prerequisite shares the any-of provider group,
    /// none is independently required, and the set is exactly the three
    /// keyed providers — DDG contributes none.
    #[test]
    fn prerequisites_are_all_grouped_under_one_provider_group() {
        let tool = WebSearchTool::default();
        let prereqs = tool.prerequisites();

        assert_eq!(
            prereqs.len(),
            3,
            "one grouped prerequisite per keyed provider, DDG contributes none"
        );
        for p in &prereqs {
            assert_eq!(
                p.group.as_deref(),
                Some(WEB_SEARCH_PROVIDER_GROUP),
                "every web_search prerequisite must share the any-of provider group, got {p:?}"
            );
            assert!(
                !p.required,
                "a grouped prerequisite must never be independently required, got {p:?}"
            );
            assert_eq!(
                p.kind, "env_var",
                "web_search prerequisites are env_var-kind, got {p:?}"
            );
        }

        let mut names: Vec<&str> = prereqs.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["BRAVE_API_KEY", "EXA_API_KEY", "TAVILY_API_KEY"],
            "web_search's grouped prerequisites must name exactly the three keyed providers"
        );
    }

    /// D-09: `satisfied_group_members` reports the count of the group's
    /// members currently satisfied, and `group_members` reports the group's
    /// full size regardless of how many are satisfied.
    #[tokio::test]
    async fn satisfied_group_members_reports_the_configured_count() {
        let _g = env_lock().lock().await;
        clear_all_provider_keys();
        // SAFETY: env_lock held, --test-threads=1 convention.
        unsafe { std::env::set_var("EXA_API_KEY", "plan07-satisfied-group-members-probe") };

        let tool = WebSearchTool::default();
        let prereqs = tool.prerequisites();

        assert_eq!(
            crate::registry::satisfied_group_members(&prereqs, WEB_SEARCH_PROVIDER_GROUP),
            1,
            "exactly one provider key (EXA_API_KEY) is configured"
        );
        assert_eq!(
            crate::registry::group_members(&prereqs, WEB_SEARCH_PROVIDER_GROUP).len(),
            3,
            "group_members must report the full keyed-provider count regardless of how many \
             are satisfied"
        );

        // SAFETY: env_lock held.
        unsafe { std::env::remove_var("EXA_API_KEY") };
    }

    /// D-08: with the first chain entry's key absent from the snapshot, the
    /// walk must skip it (no network attempt — `exa`'s endpoint is left
    /// unmocked) and fall through to the second entry, which answers.
    /// Asserts on `SearchOutcome.provider` directly rather than parsing
    /// rendered text.
    #[tokio::test]
    async fn chain_walk_falls_through_to_the_next_provider() {
        let _g = env_lock().lock().await;
        clear_all_provider_keys();

        let brave_server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
                r#"{"web":{"results":[{"title":"Fallthrough Title","url":"https://example.com/x","description":"D"}]}}"#,
            ))
            .mount(&brave_server)
            .await;

        // SAFETY: env_lock held, --test-threads=1 convention.
        unsafe {
            std::env::set_var("BRAVE_SEARCH_ENDPOINT_OVERRIDE", brave_server.uri());
        }

        let config = config_with_credential("BRAVE_API_KEY", "plan07-fallthrough-brave-key");
        let creds = ToolCredentials::resolve(&config, None)
            .await
            .expect("resolve must succeed");
        let tool = WebSearchTool::new(Arc::new(creds));

        let outcome = tool
            .search_chain(
                "rust",
                10,
                &backends::SearchFilters::default(),
                &["exa".to_string(), "brave".to_string()],
            )
            .await
            .expect("brave (second entry) must answer once exa (first, unkeyed) is skipped");

        assert_eq!(
            outcome.provider, "brave",
            "with exa unkeyed, the chain must fall through to brave"
        );

        // SAFETY: env_lock held.
        unsafe { std::env::remove_var("BRAVE_SEARCH_ENDPOINT_OVERRIDE") };
    }
}

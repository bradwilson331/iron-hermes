//! Phase 41.3 D-07/D-08 (Plan 07): `web_search` provider backends — Exa,
//! Tavily, Brave, DuckDuckGo — mirroring `web_extract/backends/`'s
//! per-provider module shape. Each keyed backend takes its API key as a
//! `&secrecy::SecretString` parameter supplied by `WebSearchTool` from its
//! resolved D-19 credential snapshot; DDG needs no key and omits the
//! parameter entirely.

pub mod brave;
pub mod ddg;
pub mod exa;
pub mod tavily;

/// Phase 41.3 D-07/D-08: optional per-call filters COVERAGE.md §1 marks
/// INTEGRATE across providers — domain include/exclude and a symbolic
/// freshness window. Each backend maps these onto its own provider-specific
/// parameter, or silently ignores a filter its provider has no analogue for
/// (see COVERAGE.md §2.1-2.5 for the per-provider decision) — this is a
/// scoping refinement, not a security boundary, so an unrecognised
/// `freshness` value is ignored rather than rejected.
///
/// The `category` (result vertical) axis was added by quick task 261015-tcs
/// — see `.planning/quick/261015-tcs-web-search-news-category-and-highlight-s/COVERAGE.md`,
/// which reverses 41.3 COVERAGE.md §1's "Result category filter" opt-out for
/// the `news` vertical only.
#[derive(Debug, Default, Clone)]
pub struct SearchFilters {
    pub include_domains: Vec<String>,
    pub exclude_domains: Vec<String>,
    /// One of `"day"` | `"week"` | `"month"` | `"year"`; anything else is
    /// silently ignored by every backend.
    pub freshness: Option<String>,
    /// Result vertical. Only `"news"` changes behavior; `"general"`, an
    /// absent value, or anything else means a normal web search. DuckDuckGo
    /// ignores it (its keyless HTML endpoint has no news vertical). Like
    /// `freshness`, an unrecognised value is ignored, not rejected: this is
    /// scoping, not a security boundary. Backends must read it only through
    /// [`SearchFilters::is_news`].
    pub category: Option<String>,
}

impl SearchFilters {
    /// The single allowlist chokepoint for the `category` axis (threat
    /// T-261015-tcs-01). Backends branch on this and emit their own
    /// hard-coded literals (`"news"`, the news endpoint constant); the
    /// caller's category string is never serialized into a provider request.
    pub fn is_news(&self) -> bool {
        self.category.as_deref() == Some("news")
    }
}

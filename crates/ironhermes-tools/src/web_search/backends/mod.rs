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
#[derive(Debug, Default, Clone)]
pub struct SearchFilters {
    pub include_domains: Vec<String>,
    pub exclude_domains: Vec<String>,
    /// One of `"day"` | `"week"` | `"month"` | `"year"`; anything else is
    /// silently ignored by every backend.
    pub freshness: Option<String>,
}

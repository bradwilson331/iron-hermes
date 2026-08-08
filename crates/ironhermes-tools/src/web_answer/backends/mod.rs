//! Phase 41.3 D-07/D-13 (Plan 08): `web_answer` provider backends —
//! Perplexity, Exa, Brave, DuckDuckGo — mirroring `web_search/backends/`'s
//! (Plan 07) per-provider module shape. Each keyed backend takes its API
//! key as a `&secrecy::SecretString` parameter supplied by `WebAnswerTool`
//! from its resolved D-19 credential snapshot; DDG needs no key and omits
//! the parameter entirely.
//!
//! Unlike `web_search/backends`, there is no shared `SearchFilters`-style
//! struct here — `web_answer`'s schema carries only `query` (D-07: no
//! domain/freshness filters, no `mode` argument, one honest contract).

pub mod brave;
pub mod ddg;
pub mod exa;
pub mod perplexity;

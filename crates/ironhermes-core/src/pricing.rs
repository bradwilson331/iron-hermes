// Phase 36.2 Plan 04 — pricing registry with static table + disk-cache overlay.
//
// Mirrors Phase 21.3's model_metadata.rs / models_cache.rs pattern. The
// resolution chain (cache > static table > provider/-prefix strip > None) is
// a direct analog of ModelRegistry::lookup (model_metadata.rs:54-91).
//
// All arithmetic is i64 integer micro-USD (1 USD = 1_000_000). NO f64
// anywhere in this module — see RESEARCH.md Pitfall 5: Float Cost Drift.
// SQLite stores `cost_usd_micros INTEGER` (Plan 02 migration). Display layer
// (Plan 10 status-bar pill) does `cost_usd_micros as f64 / 1_000_000.0` at
// presentation time only — never round-trip through f64 for storage or math.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Pricing entry for a single model — all token-rate values are micro-USD
/// integers per 1M tokens (1 USD = 1_000_000). D-USAGE-01 / RESEARCH.md Pitfall 5.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PricingEntry {
    pub provider: String,
    pub input_per_1m_micros: i64,
    pub output_per_1m_micros: i64,
    pub cache_read_per_1m_micros: i64,
    pub cache_creation_per_1m_micros: i64,
}

/// Registry with bundled static table + disk-cache overlay.
///
/// Resolution chain (mirrors `ModelRegistry::lookup` precedence per Phase 21.3 D-06):
///   1. Exact match in cache
///   2. Exact match in static table
///   3. Strip `provider/` prefix and try cache, then static table
///   4. Return None
///
/// `lookup_or_zero` gives graceful fallback to a zero-cost entry plus a
/// `tracing::warn!` — never panics, never propagates errors to the cost path.
#[derive(Debug, Clone, Default)]
pub struct PricingRegistry {
    table: HashMap<String, PricingEntry>,
    cache: HashMap<String, PricingEntry>,
}

impl PricingRegistry {
    /// Load the bootstrap table from the bundled `pricing.toml`.
    /// Falls back to an empty registry on parse failure (defensive — the
    /// bundled TOML is compile-time-known so this should never trigger).
    pub fn new() -> Self {
        let toml_str = include_str!("pricing.toml");
        Self::from_toml_str(toml_str).unwrap_or_default()
    }

    /// Parse a TOML string into a registry. The parser is **strict by shape**:
    /// the top-level body must contain a `[models.<id>]` table. Unrecognized
    /// top-level keys (e.g. `[exploit_payload]` from a spoofed refresh payload —
    /// threat T-36.2-04-PARSE) are silently dropped because they have no
    /// matching field on the internal `PricingFile` struct. The downstream
    /// invariant for Plan 09's `fetch_from_models_dev` is to additionally
    /// verify `parsed.models.len() > 0` before overwriting the disk cache.
    pub fn from_toml_str(s: &str) -> anyhow::Result<Self> {
        #[derive(Deserialize)]
        struct PricingFile {
            #[serde(default)]
            models: HashMap<String, PricingEntry>,
        }
        let parsed: PricingFile = toml::from_str(s)?;
        Ok(Self {
            table: parsed.models,
            cache: HashMap::new(),
        })
    }

    /// Resolution chain: cache exact > table exact > prefix-strip > None.
    /// Mirrors Phase 21.3 `ModelRegistry::lookup` (model_metadata.rs:54-91).
    pub fn lookup(&self, model_id: &str) -> Option<&PricingEntry> {
        // 1. Exact match in cache (overrides static)
        if let Some(e) = self.cache.get(model_id) {
            return Some(e);
        }
        // 2. Exact match in static table
        if let Some(e) = self.table.get(model_id) {
            return Some(e);
        }
        // 3. Strip `provider/` prefix -> try cache, then static table
        if let Some((_provider, bare)) = model_id.split_once('/') {
            if let Some(e) = self.cache.get(bare) {
                return Some(e);
            }
            if let Some(e) = self.table.get(bare) {
                return Some(e);
            }
        }
        // 4. Not found
        None
    }

    /// Lookup with graceful fallback: emits `tracing::warn!` and returns a
    /// zero-cost entry on miss. Never panics. The warn payload is bounded —
    /// just the `model_id` string the caller passed in (no PII surface).
    pub fn lookup_or_zero(&self, model_id: &str) -> PricingEntry {
        match self.lookup(model_id) {
            Some(e) => e.clone(),
            None => {
                tracing::warn!(model_id, "unknown model pricing — cost will be $0");
                PricingEntry {
                    provider: String::new(),
                    input_per_1m_micros: 0,
                    output_per_1m_micros: 0,
                    cache_read_per_1m_micros: 0,
                    cache_creation_per_1m_micros: 0,
                }
            }
        }
    }

    /// Merge a set of cache entries into the registry. Cache entries override
    /// static-table entries for the same key (mirrors D-06 precedence).
    /// Plan 09's `hermes pricing refresh` calls this after a successful fetch.
    pub fn merge_cache(&mut self, entries: HashMap<String, PricingEntry>) {
        self.cache.extend(entries);
    }

    /// Returns the union of static-table and cache entries, with cache winning
    /// on key collision. Sorted by canonical model_id. Used by `hermes pricing
    /// list` (Plan 09) and downstream UI surfaces.
    pub fn all_models(&self) -> Vec<(&str, &PricingEntry)> {
        let mut result: HashMap<&str, &PricingEntry> = HashMap::new();
        for (id, entry) in &self.table {
            result.insert(id.as_str(), entry);
        }
        for (id, entry) in &self.cache {
            result.insert(id.as_str(), entry);
        }
        let mut sorted: Vec<(&str, &PricingEntry)> = result.into_iter().collect();
        sorted.sort_by_key(|(id, _)| *id);
        sorted
    }
}

/// Compute the total cost in micro-USD for a single LLM turn.
///
/// **Anthropic billable input formula** (RESEARCH.md correction #9 / Pitfall 7):
/// Anthropic's `input_tokens` in the response payload counts ONLY the tokens
/// AFTER the last cache breakpoint. The total billable input is the sum
/// `input_tokens + cache_read_input_tokens + cache_creation_input_tokens`,
/// each priced at its own per-million rate. This function performs that sum
/// using integer division — no float drift, no rounding ambiguity.
///
/// Plan 07 (agent-loop usage_events writer) is responsible for passing the
/// un-summed values straight from the Usage struct; this function only sums.
///
/// All arithmetic is `i64` integer division by 1_000_000. Worst-case overflow
/// (i64 max ≈ 9.2e18) is 100,000× above realistic per-turn token×price
/// products (≈ 2e13 at 200K-token context × $100/M price) — see threat
/// T-36.2-04-OVERFLOW (accepted, documented).
pub fn compute_cost_micros(
    entry: &PricingEntry,
    in_tok: i64,
    out_tok: i64,
    cache_read: i64,
    cache_create: i64,
) -> i64 {
    let input_cost = (in_tok * entry.input_per_1m_micros) / 1_000_000;
    let output_cost = (out_tok * entry.output_per_1m_micros) / 1_000_000;
    let read_cost = (cache_read * entry.cache_read_per_1m_micros) / 1_000_000;
    let create_cost = (cache_create * entry.cache_creation_per_1m_micros) / 1_000_000;
    input_cost + output_cost + read_cost + create_cost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_loads_bundled_toml() {
        let reg = PricingRegistry::new();
        assert!(reg.lookup("claude-opus-4-7").is_some());
    }

    #[test]
    fn all_models_sorted() {
        let reg = PricingRegistry::new();
        let all = reg.all_models();
        let ids: Vec<&str> = all.iter().map(|(id, _)| *id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn from_toml_str_minimal() {
        let toml = r#"
[models."test-model"]
provider = "x"
input_per_1m_micros = 1
output_per_1m_micros = 2
cache_read_per_1m_micros = 3
cache_creation_per_1m_micros = 4
"#;
        let reg = PricingRegistry::from_toml_str(toml).expect("parse ok");
        let e = reg.lookup("test-model").expect("present");
        assert_eq!(e.input_per_1m_micros, 1);
        assert_eq!(e.output_per_1m_micros, 2);
        assert_eq!(e.cache_read_per_1m_micros, 3);
        assert_eq!(e.cache_creation_per_1m_micros, 4);
    }
}

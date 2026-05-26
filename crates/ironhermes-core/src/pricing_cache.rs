// Phase 36.2 Plan 04 — disk-cache layer for the pricing registry.
//
// Mirrors `models_cache.rs:1-87` (Phase 21.3): JSON-serialized HashMap at
// `$HERMES_HOME/pricing-cache.json`, safe load (missing/corrupt -> default),
// pretty-printed save with parent dir creation. Online fetch is wired by
// Plan 09 (`hermes pricing refresh` CLI) — this module provides the disk
// infra only.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::constants::get_hermes_home;
use crate::pricing::PricingEntry;

/// A single cached pricing entry with the fetch timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingCacheEntry {
    pub pricing: PricingEntry,
    pub fetched_at: DateTime<Utc>,
}

/// Disk-persisted cache of model pricing overlays.
///
/// On `load`, a missing or corrupt file yields `Self::default()` (empty map)
/// — never panics. On `save`, the parent directory is created if absent.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PricingCache {
    pub entries: HashMap<String, PricingCacheEntry>,
}

impl PricingCache {
    const CACHE_FILENAME: &'static str = "pricing-cache.json";

    /// Path to the cache file under `$HERMES_HOME`.
    pub fn cache_path() -> PathBuf {
        get_hermes_home().join(Self::CACHE_FILENAME)
    }

    /// Load the cache from the default `$HERMES_HOME/pricing-cache.json`.
    /// Missing file or malformed JSON both yield `Self::default()` — never panics.
    pub fn load() -> Self {
        Self::load_from(&Self::cache_path())
    }

    /// Load the cache from an arbitrary path (test override).
    pub fn load_from(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist the cache as pretty-printed JSON at the default path.
    /// Creates the parent directory if absent.
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&Self::cache_path())
    }

    /// Persist the cache to an arbitrary path (test override).
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Convert into the `HashMap<String, PricingEntry>` shape expected by
    /// `PricingRegistry::merge_cache`. Drops the `fetched_at` metadata.
    pub fn into_pricing_map(self) -> HashMap<String, PricingEntry> {
        self.entries
            .into_iter()
            .map(|(k, v)| (k, v.pricing))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Phase 36.2 Plan 09 — online refresh from models.dev
// ---------------------------------------------------------------------------

/// Hardcoded compile-time URL for `hermes pricing refresh`. Threat T-36.2-09-URL
/// (Spoofing — refresh URL forgery) mitigation: the URL is NOT a config field
/// and NOT user-overridable, so an adversary cannot redirect refresh traffic to
/// a malicious endpoint via config tampering. The const is exposed only at the
/// network boundary; everything downstream is integer micro-USD.
pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// Default per-request timeout for the models.dev fetch. Bounds latency / DoS
/// surface — see threat T-36.2-09-DOS.
const FETCH_TIMEOUT_SECS: u64 = 30;

/// Fetch the latest pricing from models.dev (production entry point used by
/// `hermes pricing refresh`).
///
/// Mirrors the shape of `models_cache::fetch_from_models_dev`: reqwest GET,
/// strict JSON parse, no leakage of f64 past `cost_field_to_micros`.
///
/// Returns a `HashMap<String, PricingCacheEntry>` keyed by canonical model id.
/// Errors propagate verbatim — the caller (`pricing_cmd::cmd_refresh`) is
/// responsible for surfacing them and preserving the existing on-disk cache.
pub async fn fetch_from_models_dev() -> anyhow::Result<HashMap<String, PricingCacheEntry>> {
    fetch_from_url(MODELS_DEV_URL).await
}

/// Test-injection seam: fetch from an arbitrary URL. Production code always
/// uses the [`MODELS_DEV_URL`] const — this seam exists so wiremock-backed
/// integration tests can drive the parser end-to-end without spoofing the
/// real DNS hostname.
pub async fn fetch_from_url(url: &str) -> anyhow::Result<HashMap<String, PricingCacheEntry>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()?;
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("models.dev returned HTTP {}", resp.status());
    }
    let body: serde_json::Value = resp.json().await?;
    Ok(parse_models_dev_body(&body))
}

/// Parse a models.dev `/api.json` response body into pricing entries.
///
/// Expected JSON shape (verified 2026-05-25):
/// ```json
/// {
///   "<provider-id>": {
///     "models": {
///       "<model-id>": {
///         "cost": {
///           "input":       <float USD per 1M tokens>,
///           "output":      <float USD per 1M tokens>,
///           "cache_read":  <float USD per 1M tokens>,
///           "cache_write": <float USD per 1M tokens>
///         }
///       }
///     }
///   }
/// }
/// ```
///
/// Threat T-36.2-09-PARSE mitigation: strict typed extraction via
/// `as_object()` / `as_f64()` / `as_i64()` / `as_u64()`. Unknown JSON shapes
/// degrade silently to an empty map; the body never feeds `format!`, `eval`,
/// or any template. Entries with all-zero input AND output pricing are
/// skipped (likely incomplete upstream data — defensive against partial
/// scrapes).
pub fn parse_models_dev_body(body: &serde_json::Value) -> HashMap<String, PricingCacheEntry> {
    let mut out = HashMap::new();
    let now = Utc::now();

    let Some(providers) = body.as_object() else {
        return out;
    };

    for (provider_id, provider_val) in providers {
        let Some(models) = provider_val.get("models").and_then(|v| v.as_object()) else {
            continue;
        };
        for (model_id, model_val) in models {
            let Some(cost) = model_val.get("cost").and_then(|v| v.as_object()) else {
                continue;
            };
            let entry = PricingEntry {
                provider: provider_id.clone(),
                input_per_1m_micros: cost_field_to_micros(cost.get("input")),
                output_per_1m_micros: cost_field_to_micros(cost.get("output")),
                cache_read_per_1m_micros: cost_field_to_micros(cost.get("cache_read")),
                cache_creation_per_1m_micros: cost_field_to_micros(cost.get("cache_write")),
            };
            // Skip entries with all-zero pricing (likely incomplete upstream data).
            if entry.input_per_1m_micros == 0 && entry.output_per_1m_micros == 0 {
                continue;
            }
            out.insert(
                model_id.clone(),
                PricingCacheEntry {
                    pricing: entry,
                    fetched_at: now,
                },
            );
        }
    }
    out
}

/// Convert a JSON numeric field (USD per 1M, possibly float) to i64 micro-USD.
///
/// This is the **ONLY** place an f64 value crosses the boundary into the
/// pricing subsystem (RESEARCH.md Pitfall 5: Float Cost Drift). After this
/// function returns, all downstream arithmetic uses `i64` micro-USD.
///
/// Behavior:
/// - `f64`  → `(v * 1_000_000) as i64`  (precision floor — matches Anthropic
///   billing rounding which never publishes sub-cent values for the price-list
///   side of cache pricing)
/// - `i64`  → `v * 1_000_000`
/// - `u64`  → `(v as i64) * 1_000_000`
/// - `None` / null / non-numeric → `0` (graceful — see threat T-36.2-09-PARSE)
pub fn cost_field_to_micros(field: Option<&serde_json::Value>) -> i64 {
    // CR-08: round on float→int conversion (was truncate-toward-zero, which
    // caused a 1% accuracy loss on cheap-model prices like $0.0001/M where
    // 0.0001 * 1_000_000.0 ≈ 99.9999... truncated to 99 instead of 100).
    // Also use checked arithmetic on i64/u64 so a malformed refresh payload
    // with an absurdly large value can't silently wrap into a negative
    // micro-USD value.
    const MAX_TOK_PRICE: i64 = i64::MAX / 1_000_000;
    match field {
        Some(v) if v.is_f64() => {
            let scaled = v.as_f64().unwrap_or(0.0) * 1_000_000.0;
            if scaled.is_finite() {
                scaled.round() as i64
            } else {
                tracing::warn!("pricing cost field is non-finite f64; treating as 0");
                0
            }
        }
        Some(v) if v.is_i64() => {
            let n = v.as_i64().unwrap_or(0);
            if n.abs() <= MAX_TOK_PRICE {
                n * 1_000_000
            } else {
                tracing::warn!("pricing cost field i64 overflows i64 micros; treating as 0");
                0
            }
        }
        Some(v) if v.is_u64() => {
            let n = v.as_u64().unwrap_or(0);
            if n <= MAX_TOK_PRICE as u64 {
                (n as i64) * 1_000_000
            } else {
                tracing::warn!("pricing cost field u64 overflows i64 micros; treating as 0");
                0
            }
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn cache_default_is_empty() {
        let c = PricingCache::default();
        assert!(c.entries.is_empty());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("pricing-cache.json");

        let mut cache = PricingCache::default();
        cache.entries.insert(
            "claude-opus-4-7".to_string(),
            PricingCacheEntry {
                pricing: PricingEntry {
                    provider: "anthropic".to_string(),
                    input_per_1m_micros: 5_000_000,
                    output_per_1m_micros: 25_000_000,
                    cache_read_per_1m_micros: 500_000,
                    cache_creation_per_1m_micros: 6_250_000,
                },
                fetched_at: Utc::now(),
            },
        );

        cache.save_to(&path).expect("save");
        let loaded = PricingCache::load_from(&path);

        assert_eq!(loaded.entries.len(), 1);
        let entry = loaded.entries.get("claude-opus-4-7").expect("entry");
        assert_eq!(entry.pricing.input_per_1m_micros, 5_000_000);
        assert_eq!(entry.pricing.provider, "anthropic");
    }

    #[test]
    fn load_from_missing_returns_default() {
        let cache = PricingCache::load_from(Path::new("/nonexistent/pricing-cache.json"));
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn load_from_corrupt_returns_default() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not valid json {{{").expect("write");
        let cache = PricingCache::load_from(&path);
        assert!(cache.entries.is_empty());
    }
}

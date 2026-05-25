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

/// Stub for Plan 09 — `hermes pricing refresh` CLI wires this up. Returning
/// `Err` keeps the surface present (so callers can `pub use` it) without
/// promising a working fetch here.
pub async fn fetch_from_models_dev() -> anyhow::Result<HashMap<String, PricingCacheEntry>> {
    anyhow::bail!("hermes pricing refresh: online fetch not yet implemented (Plan 09)")
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

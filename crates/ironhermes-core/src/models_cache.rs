// Disk cache and API fetch layer for model metadata.
// Phase 21.3 Plan 03 — supplements static lookup table with runtime-fetched metadata.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::constants::{OPENROUTER_MODELS_URL, get_hermes_home};
use crate::model_metadata::{ModelCapabilities, ModelMetadata};

/// A single cached model metadata entry with fetch timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsCacheEntry {
    pub metadata: ModelMetadata,
    pub fetched_at: DateTime<Utc>,
}

/// Disk-persisted cache of model metadata fetched from external APIs.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ModelsCache {
    pub entries: HashMap<String, ModelsCacheEntry>,
}

/// Result of a fetch_all operation, reporting success/failure for each source.
pub struct FetchResult {
    pub models_dev_count: Option<usize>,
    pub openrouter_count: Option<usize>,
    pub models_dev_error: Option<String>,
    pub openrouter_error: Option<String>,
}

const MODELS_DEV_URL: &str = "https://models.dev/api.json";

impl ModelsCache {
    const CACHE_FILENAME: &'static str = "models-cache.json";

    /// Path to the cache file on disk.
    pub fn cache_path() -> std::path::PathBuf {
        get_hermes_home().join(Self::CACHE_FILENAME)
    }

    /// Load cache from disk. Returns empty cache if file doesn't exist or is malformed.
    pub fn load() -> Self {
        let path = Self::cache_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Load cache from a specific path (used for testing and custom locations).
    pub fn load_from(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save cache to disk as pretty-printed JSON.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::cache_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    /// Save cache to a specific path (used for testing and custom locations).
    pub fn save_to(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Convert cache entries into a HashMap<String, ModelMetadata> for ModelRegistry::merge_cache.
    pub fn into_metadata_map(self) -> HashMap<String, ModelMetadata> {
        self.entries
            .into_iter()
            .map(|(k, v)| (k, v.metadata))
            .collect()
    }
}

/// Strip "provider/" prefix from model ID. Returns the bare model name.
pub fn normalize_model_id(id: &str) -> &str {
    id.split_once('/').map(|(_, bare)| bare).unwrap_or(id)
}

/// Strip date suffix (-YYYYMMDD pattern) from a model ID to get the canonical form.
fn strip_date_suffix(id: &str) -> Option<&str> {
    // Check for -YYYYMMDD at the end (9 chars: dash + 8 digits)
    if id.len() >= 10 {
        let suffix = &id[id.len() - 9..];
        if suffix.starts_with('-') && suffix[1..].chars().all(|c| c.is_ascii_digit()) {
            return Some(&id[..id.len() - 9]);
        }
    }
    None
}

/// Parse models.dev API response into cache entries.
///
/// Expected JSON structure:
/// ```json
/// { "<provider-id>": { "models": { "<model-id>": { "limit": { "context": N, "output": N }, "tool_call": bool, "reasoning": bool, "attachment": bool } } } }
/// ```
pub fn parse_models_dev_response(body: &serde_json::Value) -> HashMap<String, ModelsCacheEntry> {
    let mut result = HashMap::new();
    let now = Utc::now();

    let Some(providers) = body.as_object() else {
        return result;
    };

    for (_provider_id, provider_data) in providers {
        let Some(models) = provider_data.get("models").and_then(|m| m.as_object()) else {
            continue;
        };

        for (_model_key, model_data) in models {
            // Get the model's own ID field
            let model_id = match model_data.get("id").and_then(|v| v.as_str()) {
                Some(id) => id,
                None => continue,
            };

            // Normalize: strip provider prefix if present
            let canonical_id = normalize_model_id(model_id).to_string();

            let context_length = model_data
                .pointer("/limit/context")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            if context_length == 0 {
                continue; // Skip models without context length
            }

            let max_output_tokens = model_data
                .pointer("/limit/output")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);

            let vision = model_data
                .get("attachment")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let tool_use = model_data
                .get("tool_call")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let reasoning = model_data
                .get("reasoning")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let entry = ModelsCacheEntry {
                metadata: ModelMetadata {
                    context_length,
                    max_output_tokens,
                    // models.dev has no tokenizer field — use cl100k_base as default
                    tokenizer: "cl100k_base".to_string(),
                    capabilities: ModelCapabilities {
                        vision,
                        tool_use,
                        reasoning,
                        streaming: true, // default to true
                    },
                },
                fetched_at: now,
            };

            result.insert(canonical_id, entry);
        }
    }

    result
}

/// Map OpenRouter tokenizer name to tiktoken encoding name.
fn map_openrouter_tokenizer(tokenizer: &str, model_id: &str) -> String {
    match tokenizer {
        "Claude" => "cl100k_base".to_string(),
        "GPT" => {
            // Newer models (4o, o3, o4) use o200k_base
            let bare = normalize_model_id(model_id);
            if bare.contains("4o")
                || bare.contains("o3")
                || bare.contains("o4")
                || bare.contains("4.1")
            {
                "o200k_base".to_string()
            } else {
                "cl100k_base".to_string()
            }
        }
        _ => "cl100k_base".to_string(), // Llama3, Mistral, anything else -> cl100k (D-08 fallback)
    }
}

/// Parse OpenRouter /models API response into cache entries.
///
/// Expected JSON structure:
/// ```json
/// { "data": [{ "id": "provider/model-id", "context_length": N, "architecture": { "tokenizer": "Claude", "modality": "text+image->text" }, "top_provider": { "max_completion_tokens": N } }] }
/// ```
pub fn parse_openrouter_response(body: &serde_json::Value) -> HashMap<String, ModelsCacheEntry> {
    let mut result = HashMap::new();
    let now = Utc::now();

    let Some(data) = body.get("data").and_then(|d| d.as_array()) else {
        return result;
    };

    for model in data {
        let full_id = match model.get("id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => continue,
        };

        let context_length = model
            .get("context_length")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        if context_length == 0 {
            continue;
        }

        let max_output_tokens = model
            .pointer("/top_provider/max_completion_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let tokenizer_name = model
            .pointer("/architecture/tokenizer")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let tokenizer = map_openrouter_tokenizer(tokenizer_name, full_id);

        let modality = model
            .pointer("/architecture/modality")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let vision = modality.contains("image");

        // Derive capabilities from model ID patterns
        let bare_id = normalize_model_id(full_id);
        let reasoning = bare_id.contains("o3")
            || bare_id.contains("o4")
            || bare_id.starts_with("deepseek-r1")
            || bare_id.contains("reasoning");
        let tool_use = true; // Most models support tool use

        let entry = ModelsCacheEntry {
            metadata: ModelMetadata {
                context_length,
                max_output_tokens,
                tokenizer,
                capabilities: ModelCapabilities {
                    vision,
                    tool_use,
                    reasoning,
                    streaming: true,
                },
            },
            fetched_at: now,
        };

        // Store under canonical (prefix-stripped) ID
        let canonical = normalize_model_id(full_id).to_string();

        // Also store under date-stripped form if applicable
        if let Some(unversioned) = strip_date_suffix(&canonical) {
            // Store unversioned as primary key (most useful for lookups)
            result.insert(unversioned.to_string(), entry.clone());
        }

        // Always store the canonical (possibly versioned) form
        result.insert(canonical, entry);
    }

    result
}

/// Fetch model metadata from models.dev API.
/// Returns a map of canonical_id -> ModelsCacheEntry.
pub async fn fetch_from_models_dev() -> anyhow::Result<HashMap<String, ModelsCacheEntry>> {
    let client = reqwest::Client::new();
    let resp = client
        .get(MODELS_DEV_URL)
        .send()
        .await?
        .error_for_status()?;
    let body: serde_json::Value = resp.json().await?;
    Ok(parse_models_dev_response(&body))
}

/// Fetch model metadata from OpenRouter /models API.
/// Optionally uses OPENROUTER_API_KEY env var for authenticated access.
pub async fn fetch_from_openrouter() -> anyhow::Result<HashMap<String, ModelsCacheEntry>> {
    let client = reqwest::Client::new();
    let mut req = client.get(OPENROUTER_MODELS_URL);
    if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
        req = req.bearer_auth(key);
    }
    let resp = req.send().await?.error_for_status()?;
    let body: serde_json::Value = resp.json().await?;
    Ok(parse_openrouter_response(&body))
}

/// Fetch from models.dev first, merge OpenRouter results on top.
/// OpenRouter has richer tokenizer info, so its entries win when both provide the same model.
/// Returns (merged_entries, FetchResult) where FetchResult reports what succeeded/failed.
pub async fn fetch_all() -> (HashMap<String, ModelsCacheEntry>, FetchResult) {
    let mut merged = HashMap::new();
    let mut result = FetchResult {
        models_dev_count: None,
        openrouter_count: None,
        models_dev_error: None,
        openrouter_error: None,
    };

    // Try models.dev first
    match fetch_from_models_dev().await {
        Ok(entries) => {
            result.models_dev_count = Some(entries.len());
            merged.extend(entries);
        }
        Err(e) => {
            result.models_dev_error = Some(e.to_string());
        }
    }

    // Try OpenRouter (even if models.dev succeeded — OpenRouter adds tokenizer data)
    match fetch_from_openrouter().await {
        Ok(entries) => {
            result.openrouter_count = Some(entries.len());
            // OpenRouter entries override models.dev for same key (richer data)
            merged.extend(entries);
        }
        Err(e) => {
            result.openrouter_error = Some(e.to_string());
        }
    }

    (merged, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_entry_serde_roundtrip() {
        let entry = ModelsCacheEntry {
            metadata: ModelMetadata {
                context_length: 200_000,
                max_output_tokens: Some(64_000),
                tokenizer: "cl100k_base".to_string(),
                capabilities: ModelCapabilities {
                    vision: true,
                    tool_use: true,
                    reasoning: false,
                    streaming: true,
                },
            },
            fetched_at: Utc::now(),
        };

        let json = serde_json::to_string(&entry).expect("serialize");
        let decoded: ModelsCacheEntry = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded.metadata.context_length, 200_000);
        assert_eq!(decoded.metadata.max_output_tokens, Some(64_000));
        assert_eq!(decoded.metadata.tokenizer, "cl100k_base");
        assert!(decoded.metadata.capabilities.vision);
        assert!(decoded.metadata.capabilities.tool_use);
        assert!(!decoded.metadata.capabilities.reasoning);
        assert!(decoded.metadata.capabilities.streaming);
    }

    #[test]
    fn models_cache_default_is_empty() {
        let cache = ModelsCache::default();
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("models-cache.json");

        let mut cache = ModelsCache::default();
        cache.entries.insert(
            "claude-sonnet-4".to_string(),
            ModelsCacheEntry {
                metadata: ModelMetadata {
                    context_length: 200_000,
                    max_output_tokens: Some(64_000),
                    tokenizer: "cl100k_base".to_string(),
                    capabilities: ModelCapabilities::default(),
                },
                fetched_at: Utc::now(),
            },
        );

        cache.save_to(&path).expect("save");
        let loaded = ModelsCache::load_from(&path);

        assert_eq!(loaded.entries.len(), 1);
        let entry = loaded.entries.get("claude-sonnet-4").expect("entry");
        assert_eq!(entry.metadata.context_length, 200_000);
        assert_eq!(entry.metadata.max_output_tokens, Some(64_000));
    }

    #[test]
    fn parse_models_dev_response_extracts_metadata() {
        let json = serde_json::json!({
            "anthropic": {
                "id": "anthropic",
                "name": "Anthropic",
                "models": {
                    "claude-sonnet-4": {
                        "id": "claude-sonnet-4",
                        "name": "Claude Sonnet 4",
                        "family": "claude",
                        "attachment": true,
                        "reasoning": false,
                        "tool_call": true,
                        "limit": {
                            "context": 200000,
                            "output": 64000
                        }
                    }
                }
            }
        });

        let entries = parse_models_dev_response(&json);
        let entry = entries.get("claude-sonnet-4").expect("claude-sonnet-4");

        assert_eq!(entry.metadata.context_length, 200_000);
        assert_eq!(entry.metadata.max_output_tokens, Some(64_000));
        assert_eq!(entry.metadata.tokenizer, "cl100k_base"); // default, no tokenizer from models.dev
        assert!(entry.metadata.capabilities.vision);
        assert!(entry.metadata.capabilities.tool_use);
        assert!(!entry.metadata.capabilities.reasoning);
        assert!(entry.metadata.capabilities.streaming);
    }

    #[test]
    fn parse_openrouter_response_extracts_metadata() {
        let json = serde_json::json!({
            "data": [{
                "id": "anthropic/claude-sonnet-4-20250514",
                "name": "Claude Sonnet 4",
                "context_length": 200000,
                "architecture": {
                    "tokenizer": "Claude",
                    "modality": "text+image->text"
                },
                "top_provider": {
                    "max_completion_tokens": 64000
                }
            }]
        });

        let entries = parse_openrouter_response(&json);

        // Should have both versioned and unversioned keys
        let entry = entries
            .get("claude-sonnet-4")
            .expect("unversioned canonical key");
        assert_eq!(entry.metadata.context_length, 200_000);
        assert_eq!(entry.metadata.max_output_tokens, Some(64_000));
        assert_eq!(entry.metadata.tokenizer, "cl100k_base"); // "Claude" -> "cl100k_base"
        assert!(entry.metadata.capabilities.vision); // "text+image->text" contains "image"

        // Versioned key also present
        let versioned = entries
            .get("claude-sonnet-4-20250514")
            .expect("versioned key");
        assert_eq!(versioned.metadata.context_length, 200_000);
    }

    #[test]
    fn parse_openrouter_maps_gpt_tokenizer() {
        let json = serde_json::json!({
            "data": [
                {
                    "id": "openai/gpt-4o",
                    "context_length": 128000,
                    "architecture": {
                        "tokenizer": "GPT",
                        "modality": "text+image->text"
                    },
                    "top_provider": {
                        "max_completion_tokens": 16384
                    }
                },
                {
                    "id": "openai/gpt-4-turbo",
                    "context_length": 128000,
                    "architecture": {
                        "tokenizer": "GPT",
                        "modality": "text->text"
                    },
                    "top_provider": {
                        "max_completion_tokens": 4096
                    }
                }
            ]
        });

        let entries = parse_openrouter_response(&json);

        // GPT-4o uses o200k_base
        let gpt4o = entries.get("gpt-4o").expect("gpt-4o");
        assert_eq!(gpt4o.metadata.tokenizer, "o200k_base");

        // GPT-4-turbo uses cl100k_base (no 4o/o3/o4 in name)
        let gpt4_turbo = entries.get("gpt-4-turbo").expect("gpt-4-turbo");
        assert_eq!(gpt4_turbo.metadata.tokenizer, "cl100k_base");
    }

    #[test]
    fn normalize_model_id_strips_provider_prefix() {
        assert_eq!(
            normalize_model_id("anthropic/claude-sonnet-4"),
            "claude-sonnet-4"
        );
        assert_eq!(normalize_model_id("openai/gpt-4o"), "gpt-4o");
        assert_eq!(normalize_model_id("claude-sonnet-4"), "claude-sonnet-4"); // no prefix
        assert_eq!(
            normalize_model_id("meta-llama/llama-3.1-8b"),
            "llama-3.1-8b"
        );
    }

    #[test]
    fn cache_entries_use_canonical_ids_as_keys() {
        let json = serde_json::json!({
            "anthropic": {
                "id": "anthropic",
                "models": {
                    "claude-sonnet-4": {
                        "id": "anthropic/claude-sonnet-4",
                        "limit": { "context": 200000, "output": 64000 },
                        "tool_call": true,
                        "reasoning": false,
                        "attachment": true
                    }
                }
            }
        });

        let entries = parse_models_dev_response(&json);
        // Key should be "claude-sonnet-4" (prefix stripped), NOT "anthropic/claude-sonnet-4"
        assert!(entries.contains_key("claude-sonnet-4"));
        assert!(!entries.contains_key("anthropic/claude-sonnet-4"));
    }

    #[test]
    fn into_metadata_map_strips_cache_entry_wrapper() {
        let mut cache = ModelsCache::default();
        cache.entries.insert(
            "test-model".to_string(),
            ModelsCacheEntry {
                metadata: ModelMetadata {
                    context_length: 100_000,
                    max_output_tokens: None,
                    tokenizer: "cl100k_base".to_string(),
                    capabilities: ModelCapabilities::default(),
                },
                fetched_at: Utc::now(),
            },
        );

        let map = cache.into_metadata_map();
        assert_eq!(map.len(), 1);
        let meta = map.get("test-model").expect("test-model");
        assert_eq!(meta.context_length, 100_000);
    }

    #[test]
    fn strip_date_suffix_works() {
        assert_eq!(
            strip_date_suffix("claude-sonnet-4-20250514"),
            Some("claude-sonnet-4")
        );
        assert_eq!(strip_date_suffix("gpt-4o-2024-11-20"), None); // hyphens in date part
        assert_eq!(strip_date_suffix("claude-sonnet-4"), None); // no date suffix
        assert_eq!(strip_date_suffix("short"), None); // too short
    }

    #[test]
    fn load_from_nonexistent_returns_empty() {
        let cache = ModelsCache::load_from(std::path::Path::new("/nonexistent/path/cache.json"));
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn load_from_malformed_returns_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bad-cache.json");
        std::fs::write(&path, "not valid json {{{").expect("write");

        let cache = ModelsCache::load_from(&path);
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn parse_models_dev_skips_zero_context_length() {
        let json = serde_json::json!({
            "provider": {
                "id": "test",
                "models": {
                    "no-context": {
                        "id": "no-context",
                        "limit": { "context": 0 },
                        "tool_call": false,
                        "reasoning": false,
                        "attachment": false
                    }
                }
            }
        });

        let entries = parse_models_dev_response(&json);
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_openrouter_llama_tokenizer_defaults_to_cl100k() {
        let json = serde_json::json!({
            "data": [{
                "id": "meta-llama/llama-3.1-70b-instruct",
                "context_length": 128000,
                "architecture": {
                    "tokenizer": "Llama3",
                    "modality": "text->text"
                },
                "top_provider": {
                    "max_completion_tokens": 4096
                }
            }]
        });

        let entries = parse_openrouter_response(&json);
        let entry = entries.get("llama-3.1-70b-instruct").expect("llama model");
        assert_eq!(entry.metadata.tokenizer, "cl100k_base"); // Llama3 -> cl100k_base fallback
        assert!(!entry.metadata.capabilities.vision); // "text->text" has no "image"
    }
}

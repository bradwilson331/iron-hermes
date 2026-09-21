// Disk cache and API fetch layer for model metadata.
// Phase 21.3 Plan 03 — supplements static lookup table with runtime-fetched metadata.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::constants::{OPENROUTER_MODELS_URL, get_hermes_home};
use crate::model_metadata::{ModelCapabilities, PartialModelMetadata};

/// A single cached model metadata entry with fetch timestamp.
///
/// `metadata` is a [`PartialModelMetadata`] (Phase 50.5, D-10): a source may
/// observe only some fields (e.g. a provider `/models` probe sees only
/// `context_length`), and the registry backfills the rest from the static
/// table via `ModelRegistry::merge_partial_cache` rather than the parser
/// fabricating them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsCacheEntry {
    pub metadata: PartialModelMetadata,
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
    /// Phase 50.5 (D-08/D-09): one outcome per configured, enabled provider
    /// with a `base_url` that `fetch_all` probed. Reported the same way as
    /// the two curated sources above, but per-provider since there can be
    /// any number of them.
    pub provider_probes: Vec<ProviderProbeOutcome>,
}

/// Outcome of probing a single configured provider's own `/models` endpoint
/// (Phase 50.5, D-08/D-11). `error` is `Some` on any failure (network,
/// non-2xx, unparseable body) — a single provider's failure is reported
/// here, never propagated to fail the whole refresh. `drifted_ids` names
/// configured model ids ([`ProviderConfig::default_model`] plus
/// `ProviderConfig::models` keys) absent from this probe's served id list
/// (D-11); always empty when `error` is `Some` or the probe served nothing.
#[derive(Debug, Clone)]
pub struct ProviderProbeOutcome {
    pub provider: String,
    pub model_count: Option<usize>,
    pub error: Option<String>,
    pub drifted_ids: Vec<String>,
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

    /// Convert cache entries into a HashMap<String, PartialModelMetadata> for
    /// ModelRegistry::merge_partial_cache (Phase 50.5, D-10).
    pub fn into_partial_metadata_map(self) -> HashMap<String, PartialModelMetadata> {
        self.entries
            .into_iter()
            .map(|(k, v)| (k, v.metadata))
            .collect()
    }

    /// Load-side merge (Phase 50.5, D-08's named trap): overlays `fresh` onto
    /// the EXISTING entries via [`overlay_partial_entry`] rather than
    /// replacing them, so `/models refresh` can only grow or update the
    /// cache — never erase an entry another surface (e.g. the web harvest)
    /// already wrote. For a key present in both, only the fields `fresh`
    /// actually observed change; a key absent from `fresh` is untouched.
    pub fn merge_entries(&mut self, fresh: HashMap<String, ModelsCacheEntry>) {
        overlay_cache_entries(&mut self.entries, fresh);
    }
}

/// Per-field provider-wins overlay (Phase 50.5, D-09): `fresh`'s observed
/// fields (`Some`) win; a field `fresh` did NOT observe (`None`) falls
/// through to `base`'s value. `fresh` can update a field, it can never blank
/// one — this is what makes D-09 mean "provider wins on what it saw" rather
/// than "provider wins, full stop."
fn overlay_partial_entry(
    base: &PartialModelMetadata,
    fresh: &PartialModelMetadata,
) -> PartialModelMetadata {
    PartialModelMetadata {
        context_length: fresh.context_length.or(base.context_length),
        max_output_tokens: fresh.max_output_tokens.or(base.max_output_tokens),
        tokenizer: fresh.tokenizer.clone().or_else(|| base.tokenizer.clone()),
        capabilities: fresh
            .capabilities
            .clone()
            .or_else(|| base.capabilities.clone()),
    }
}

/// Overlays `fresh` cache entries onto `target` in place: a key present in
/// both is merged field-by-field via [`overlay_partial_entry`] (keeping
/// `fresh`'s `fetched_at`); a key absent from `target` is inserted as-is.
/// Shared by [`ModelsCache::merge_entries`] (the on-disk load-merge-save
/// path) and `fetch_all`'s provider-probe stage (the in-memory merge before
/// the curated-source map is returned to the caller).
fn overlay_cache_entries(
    target: &mut HashMap<String, ModelsCacheEntry>,
    fresh: HashMap<String, ModelsCacheEntry>,
) {
    for (id, fresh_entry) in fresh {
        match target.get(&id) {
            Some(existing) => {
                let metadata = overlay_partial_entry(&existing.metadata, &fresh_entry.metadata);
                target.insert(
                    id,
                    ModelsCacheEntry {
                        metadata,
                        fetched_at: fresh_entry.fetched_at,
                    },
                );
            }
            None => {
                target.insert(id, fresh_entry);
            }
        }
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
                metadata: PartialModelMetadata {
                    context_length: Some(context_length),
                    max_output_tokens,
                    // models.dev has no tokenizer field (Phase 50.5, D-10): leave
                    // it unobserved and let the registry backfill from the static
                    // table, rather than fabricating cl100k_base here.
                    tokenizer: None,
                    capabilities: Some(ModelCapabilities {
                        vision,
                        tool_use,
                        reasoning,
                        streaming: true, // default to true
                    }),
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
            metadata: PartialModelMetadata {
                context_length: Some(context_length),
                max_output_tokens,
                // OpenRouter genuinely observes a tokenizer for every model
                // (Phase 50.5, D-10) — nothing becomes None here.
                tokenizer: Some(tokenizer),
                capabilities: Some(ModelCapabilities {
                    vision,
                    tool_use,
                    reasoning,
                    streaming: true,
                }),
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

/// Parse a provider's own OpenAI-compatible `/models` response (Phase 50.5,
/// D-06/D-09).
///
/// Expected JSON structure:
/// ```json
/// { "data": [{ "id": "k3", "context_length": 1048576 }, ...] }
/// ```
///
/// Returns the served id list (response order, non-string ids skipped) and a
/// partial-metadata map observing ONLY `context_length` — D-10's
/// no-fabrication rule means this parser sets no other field; a probe
/// genuinely observes nothing else. Every candidate is checked against
/// [`crate::constants::MAX_PLAUSIBLE_CONTEXT_LENGTH`] and a `> 0` floor
/// (T-50.5-01) before it is accepted; a rejected value is logged and the id
/// contributes no cache entry, though it still appears in the served list —
/// what a provider serves and what window it reports are separate questions.
pub fn parse_provider_models_response(
    body: &serde_json::Value,
) -> (Vec<String>, HashMap<String, PartialModelMetadata>) {
    let mut ids = Vec::new();
    let mut partials = HashMap::new();

    let Some(data) = body.get("data").and_then(|d| d.as_array()) else {
        return (ids, partials);
    };

    for entry in data {
        let Some(id) = entry.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        ids.push(id.to_string());

        let Some(candidate) = entry.get("context_length").and_then(|v| v.as_u64()) else {
            continue;
        };
        let candidate = candidate as usize;

        if candidate == 0 || candidate > crate::constants::MAX_PLAUSIBLE_CONTEXT_LENGTH {
            tracing::warn!(
                model_id = %id,
                candidate,
                "rejected implausible context_length from provider /models probe"
            );
            continue;
        }

        partials.insert(
            id.to_string(),
            PartialModelMetadata {
                context_length: Some(candidate),
                max_output_tokens: None,
                tokenizer: None,
                capabilities: None,
            },
        );
    }

    (ids, partials)
}

/// Probe a single configured provider's own `/models` endpoint (Phase 50.5,
/// D-06/D-08/D-09).
///
/// Builds `{base_url_trimmed}/models`, attaches a bearer token resolved from
/// `cfg.api_key_env` (preferred) falling back to the deprecated `cfg.api_key`
/// literal, and wraps [`parse_provider_models_response`]'s partials into
/// [`ModelsCacheEntry`] values stamped with the current time. The resolved
/// credential is used ONLY as the outbound bearer — it is never returned,
/// logged, or interpolated into the returned error (T-50.5-11).
pub async fn fetch_from_provider(
    provider_name: &str,
    cfg: &crate::config::ProviderConfig,
) -> anyhow::Result<(Vec<String>, HashMap<String, ModelsCacheEntry>)> {
    let base_url = cfg
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("provider '{provider_name}' has no base_url configured"))?
        .trim_end_matches('/');

    let credential = cfg
        .api_key_env
        .as_deref()
        .and_then(|name| std::env::var(name).ok())
        .or_else(|| cfg.api_key.clone());

    let url = format!("{base_url}/models");
    let client = reqwest::Client::new();
    let mut req = client.get(&url);
    if let Some(bearer) = credential {
        req = req.bearer_auth(bearer);
    }

    let resp = req.send().await?.error_for_status()?;
    let body: serde_json::Value = resp.json().await?;

    let (ids, partials) = parse_provider_models_response(&body);
    let now = Utc::now();
    let entries = partials
        .into_iter()
        .map(|(id, metadata)| (id, ModelsCacheEntry { metadata, fetched_at: now }))
        .collect();

    Ok((ids, entries))
}

/// Configured model ids absent from a provider's served id list (Phase 50.5,
/// D-11) — pure, so it is unit-testable without a network call. An EMPTY
/// `served_ids` (an unreachable or non-listing provider) always returns an
/// empty vec; a provider that cannot be asked must never be reported as
/// drifting.
pub fn configured_id_drift(served_ids: &[String], configured_ids: &[String]) -> Vec<String> {
    if served_ids.is_empty() {
        return Vec::new();
    }
    configured_ids
        .iter()
        .filter(|id| !served_ids.iter().any(|served| served == *id))
        .cloned()
        .collect()
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

/// Fetch from models.dev first, merge OpenRouter results on top, then probe
/// each configured provider's own `/models` endpoint (Phase 50.5, D-08).
/// OpenRouter has richer tokenizer info, so its entries win when both provide
/// the same model over the two curated sources; a provider's own probe then
/// wins over BOTH curated sources per-field (D-09), via
/// [`overlay_cache_entries`] rather than a blind `extend` — a probe observing
/// only `context_length` must not blank a curated tokenizer.
///
/// Providers are probed SERIALLY (discretion resolution, D-08): the provider
/// count is small, this keeps `FetchResult::provider_probes` ordering
/// deterministic for the CLI's line-by-line report, and it avoids a second
/// concurrency primitive next to two already-sequential curated fetches.
/// Skips any provider that is `disabled` or has no non-empty `base_url`. One
/// provider's failure is reported in its own [`ProviderProbeOutcome`] and
/// never fails the whole refresh.
///
/// Returns (merged_entries, FetchResult) where FetchResult reports what succeeded/failed.
pub async fn fetch_all(config: &crate::config::Config) -> (HashMap<String, ModelsCacheEntry>, FetchResult) {
    let mut merged = HashMap::new();
    let mut result = FetchResult {
        models_dev_count: None,
        openrouter_count: None,
        models_dev_error: None,
        openrouter_error: None,
        provider_probes: Vec::new(),
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

    // Probe each configured, enabled provider with a base_url (D-08/D-09).
    // Sorted for deterministic ordering — `config.providers` is a HashMap.
    let mut provider_names: Vec<&String> = config.providers.keys().collect();
    provider_names.sort();

    for name in provider_names {
        let cfg = &config.providers[name];
        if cfg.disabled == Some(true) {
            continue;
        }
        let has_base_url = cfg
            .base_url
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        if !has_base_url {
            continue;
        }

        match fetch_from_provider(name, cfg).await {
            Ok((served_ids, fresh_entries)) => {
                let model_count = served_ids.len();
                overlay_cache_entries(&mut merged, fresh_entries);

                let mut configured_ids: Vec<String> = cfg.models.keys().cloned().collect();
                if let Some(default_model) = cfg.default_model.as_ref() {
                    configured_ids.push(default_model.clone());
                }
                let drifted_ids = configured_id_drift(&served_ids, &configured_ids);
                for drifted in &drifted_ids {
                    tracing::warn!(
                        provider = %name,
                        configured_id = %drifted,
                        served_ids = ?served_ids,
                        "configured model id not found in provider's served /models list"
                    );
                }

                result.provider_probes.push(ProviderProbeOutcome {
                    provider: name.clone(),
                    model_count: Some(model_count),
                    error: None,
                    drifted_ids,
                });
            }
            Err(e) => {
                result.provider_probes.push(ProviderProbeOutcome {
                    provider: name.clone(),
                    model_count: None,
                    error: Some(e.to_string()),
                    drifted_ids: Vec::new(),
                });
            }
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
            metadata: PartialModelMetadata {
                context_length: Some(200_000),
                max_output_tokens: Some(64_000),
                tokenizer: Some("cl100k_base".to_string()),
                capabilities: Some(ModelCapabilities {
                    vision: true,
                    tool_use: true,
                    reasoning: false,
                    streaming: true,
                }),
            },
            fetched_at: Utc::now(),
        };

        let json = serde_json::to_string(&entry).expect("serialize");
        let decoded: ModelsCacheEntry = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded.metadata.context_length, Some(200_000));
        assert_eq!(decoded.metadata.max_output_tokens, Some(64_000));
        assert_eq!(decoded.metadata.tokenizer, Some("cl100k_base".to_string()));
        let caps = decoded.metadata.capabilities.expect("capabilities");
        assert!(caps.vision);
        assert!(caps.tool_use);
        assert!(!caps.reasoning);
        assert!(caps.streaming);
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
                metadata: PartialModelMetadata {
                    context_length: Some(200_000),
                    max_output_tokens: Some(64_000),
                    tokenizer: Some("cl100k_base".to_string()),
                    capabilities: Some(ModelCapabilities::default()),
                },
                fetched_at: Utc::now(),
            },
        );

        cache.save_to(&path).expect("save");
        let loaded = ModelsCache::load_from(&path);

        assert_eq!(loaded.entries.len(), 1);
        let entry = loaded.entries.get("claude-sonnet-4").expect("entry");
        assert_eq!(entry.metadata.context_length, Some(200_000));
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

        assert_eq!(entry.metadata.context_length, Some(200_000));
        assert_eq!(entry.metadata.max_output_tokens, Some(64_000));
        // Phase 50.5 (D-10): inverted from a fabricated "cl100k_base" default to
        // None — models.dev carries no tokenizer field, and the registry (not
        // this parser) now supplies the fallback via merge_partial_cache.
        assert_eq!(entry.metadata.tokenizer, None);
        let caps = entry.metadata.capabilities.clone().expect("capabilities");
        assert!(caps.vision);
        assert!(caps.tool_use);
        assert!(!caps.reasoning);
        assert!(caps.streaming);
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
        assert_eq!(entry.metadata.context_length, Some(200_000));
        assert_eq!(entry.metadata.max_output_tokens, Some(64_000));
        assert_eq!(entry.metadata.tokenizer, Some("cl100k_base".to_string())); // "Claude" -> "cl100k_base"
        assert!(entry.metadata.capabilities.clone().expect("capabilities").vision); // "text+image->text" contains "image"

        // Versioned key also present
        let versioned = entries
            .get("claude-sonnet-4-20250514")
            .expect("versioned key");
        assert_eq!(versioned.metadata.context_length, Some(200_000));
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
        assert_eq!(gpt4o.metadata.tokenizer, Some("o200k_base".to_string()));

        // GPT-4-turbo uses cl100k_base (no 4o/o3/o4 in name)
        let gpt4_turbo = entries.get("gpt-4-turbo").expect("gpt-4-turbo");
        assert_eq!(gpt4_turbo.metadata.tokenizer, Some("cl100k_base".to_string()));
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
    fn into_partial_metadata_map_strips_cache_entry_wrapper() {
        let mut cache = ModelsCache::default();
        cache.entries.insert(
            "test-model".to_string(),
            ModelsCacheEntry {
                metadata: PartialModelMetadata {
                    context_length: Some(100_000),
                    max_output_tokens: None,
                    tokenizer: Some("cl100k_base".to_string()),
                    capabilities: Some(ModelCapabilities::default()),
                },
                fetched_at: Utc::now(),
            },
        );

        let map = cache.into_partial_metadata_map();
        assert_eq!(map.len(), 1);
        let meta = map.get("test-model").expect("test-model");
        assert_eq!(meta.context_length, Some(100_000));
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

    /// Phase 50.5 (D-10) migration proof: the exact pre-50.5 on-disk shape,
    /// where `metadata` carried all four `ModelMetadata` fields present and
    /// non-null. The distinctive model key appears nowhere in the static
    /// table, so a passing assertion cannot be satisfied by registry fallback.
    const PRE_MIGRATION_CACHE_JSON: &str = r#"{
        "entries": {
            "pre-50-5-fixture-model": {
                "metadata": {
                    "context_length": 123456,
                    "max_output_tokens": 4096,
                    "tokenizer": "cl100k_base",
                    "capabilities": {
                        "vision": true,
                        "tool_use": true,
                        "reasoning": false,
                        "streaming": true
                    }
                },
                "fetched_at": "2026-01-01T00:00:00Z"
            }
        }
    }"#;

    #[test]
    fn pre_migration_cache_file_still_deserializes() {
        // `.expect` — NOT `unwrap_or_default` — so a schema break is a hard
        // test failure rather than an empty map that assertions might
        // accidentally tolerate (VALIDATION Wave 0 gap 1).
        let cache: ModelsCache = serde_json::from_str(PRE_MIGRATION_CACHE_JSON)
            .expect("pre-50.5 cache JSON must still deserialize into ModelsCache");

        let entry = cache
            .entries
            .get("pre-50-5-fixture-model")
            .expect("fixture key must survive the parse");

        assert_eq!(entry.metadata.context_length, Some(123_456));
        assert_eq!(entry.metadata.max_output_tokens, Some(4_096));
        assert_eq!(entry.metadata.tokenizer, Some("cl100k_base".to_string()));
        let caps = entry
            .metadata
            .capabilities
            .clone()
            .expect("capabilities must survive the parse");
        assert!(caps.vision);
        assert!(caps.tool_use);
        assert!(!caps.reasoning);
        assert!(caps.streaming);
    }

    #[test]
    fn pre_migration_cache_file_survives_load_from() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("models-cache.json");
        std::fs::write(&path, PRE_MIGRATION_CACHE_JSON).expect("write fixture");

        // This is the assertion that actually rules out the silent-discard
        // mode — `from_str` alone would not, because production reads go
        // through `load_from`, whose `unwrap_or_default` (see `load`/
        // `load_from` above) would present a schema break as an empty cache.
        let cache = ModelsCache::load_from(&path);

        assert!(
            !cache.entries.is_empty(),
            "load_from must NOT return ModelsCache::default() for a valid pre-50.5 file"
        );
        assert!(cache.entries.contains_key("pre-50-5-fixture-model"));
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
        assert_eq!(entry.metadata.tokenizer, Some("cl100k_base".to_string())); // Llama3 -> cl100k_base fallback
        assert!(!entry.metadata.capabilities.clone().expect("capabilities").vision); // "text->text" has no "image"
    }

    // -------------------------------------------------------------------
    // Phase 50.5 Plan 04: provider /models probe, overlay, drift detection
    // -------------------------------------------------------------------

    #[test]
    fn parse_provider_models_response_extracts_windows() {
        let json = serde_json::json!({
            "data": [
                { "id": "k3", "context_length": 1_048_576 },
                { "id": "k3-256k", "context_length": 262_144 },
                { "id": "kimi-for-coding", "context_length": 262_144 }
            ]
        });

        let (ids, partials) = parse_provider_models_response(&json);

        assert_eq!(ids, vec!["k3", "k3-256k", "kimi-for-coding"]);
        assert_eq!(partials.len(), 3);
        assert_eq!(
            partials.get("k3").expect("k3").context_length,
            Some(1_048_576)
        );
        assert!(partials["k3"].tokenizer.is_none());
        assert!(partials["k3"].max_output_tokens.is_none());
        assert!(partials["k3"].capabilities.is_none());
    }

    #[test]
    fn parse_provider_models_response_with_no_context_length_yields_ids_but_no_entries() {
        let json = serde_json::json!({
            "data": [{ "id": "prov-a" }, { "id": "prov-b" }]
        });

        let (ids, partials) = parse_provider_models_response(&json);

        assert_eq!(ids, vec!["prov-a", "prov-b"]);
        assert!(partials.is_empty());
    }

    #[test]
    fn implausible_probe_window_is_discarded() {
        let json = serde_json::json!({
            "data": [
                { "id": "zero-window", "context_length": 0 },
                { "id": "huge-window", "context_length": 18_446_744_073_709_551_615u64 }
            ]
        });

        let (ids, partials) = parse_provider_models_response(&json);

        // Both still appear in the served list — a rejected window is a
        // separate question from whether the provider serves the id.
        assert_eq!(ids, vec!["zero-window", "huge-window"]);
        // Neither produces a cache entry, and neither is clamped.
        assert!(partials.is_empty());
    }

    #[test]
    fn provider_probe_wins_per_field_over_curated() {
        let curated = PartialModelMetadata {
            context_length: Some(200_000),
            max_output_tokens: Some(8_000),
            tokenizer: Some("cl100k_base".to_string()),
            capabilities: Some(ModelCapabilities::default()),
        };
        let probe = PartialModelMetadata {
            context_length: Some(1_048_576),
            max_output_tokens: None,
            tokenizer: None,
            capabilities: None,
        };

        let merged = overlay_partial_entry(&curated, &probe);

        assert_eq!(merged.context_length, Some(1_048_576));
        // Unobserved fields fall through to the curated base — never blanked.
        assert_eq!(merged.max_output_tokens, Some(8_000));
        assert_eq!(merged.tokenizer, Some("cl100k_base".to_string()));
        assert!(merged.capabilities.is_some());
    }

    #[test]
    fn merge_entries_overlays_without_erasing_untouched_keys() {
        let mut cache = ModelsCache::default();
        cache.entries.insert(
            "alpha".to_string(),
            ModelsCacheEntry {
                metadata: PartialModelMetadata {
                    context_length: Some(50_000),
                    ..Default::default()
                },
                fetched_at: Utc::now(),
            },
        );
        cache.entries.insert(
            "beta".to_string(),
            ModelsCacheEntry {
                metadata: PartialModelMetadata {
                    context_length: Some(200_000),
                    tokenizer: Some("cl100k_base".to_string()),
                    ..Default::default()
                },
                fetched_at: Utc::now(),
            },
        );

        let mut fresh = HashMap::new();
        fresh.insert(
            "beta".to_string(),
            ModelsCacheEntry {
                metadata: PartialModelMetadata {
                    context_length: Some(1_048_576),
                    ..Default::default()
                },
                fetched_at: Utc::now(),
            },
        );
        fresh.insert(
            "gamma".to_string(),
            ModelsCacheEntry {
                metadata: PartialModelMetadata {
                    context_length: Some(32_000),
                    ..Default::default()
                },
                fetched_at: Utc::now(),
            },
        );

        cache.merge_entries(fresh);

        assert_eq!(cache.entries.len(), 3);
        assert_eq!(cache.entries["alpha"].metadata.context_length, Some(50_000));
        assert_eq!(cache.entries["beta"].metadata.context_length, Some(1_048_576));
        // beta's tokenizer, unobserved by the fresh entry, survives the merge.
        assert_eq!(
            cache.entries["beta"].metadata.tokenizer,
            Some("cl100k_base".to_string())
        );
        assert_eq!(cache.entries["gamma"].metadata.context_length, Some(32_000));
    }

    #[test]
    fn configured_id_drift_detects_the_moonshot_case() {
        let served = vec!["k3".to_string(), "k3-256k".to_string()];
        let configured = vec!["kimi-k3".to_string()];

        assert_eq!(
            configured_id_drift(&served, &configured),
            vec!["kimi-k3".to_string()]
        );
    }

    #[test]
    fn configured_id_drift_matches_when_ids_agree() {
        let served = vec!["k3".to_string()];
        let configured = vec!["k3".to_string()];

        assert!(configured_id_drift(&served, &configured).is_empty());
    }

    #[test]
    fn configured_id_drift_empty_served_list_never_reports_drift() {
        let served: Vec<String> = Vec::new();
        let configured = vec!["kimi-k3".to_string()];

        assert!(configured_id_drift(&served, &configured).is_empty());
    }
}

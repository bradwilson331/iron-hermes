//! Typed per-provider `extra_request_options` configuration structs.
//!
//! # D-01 DEVIATION: Raw HashMap used at the config/deserialization boundary
//!
//! The original plan (D-01) called for a `#[serde(untagged)] enum ProviderExtraOptions` on
//! `ProviderConfig.extra_request_options`. This approach was tested (Wave 0 canary) and **failed**
//! for the OpenRouter variant: serde_yaml with untagged enums and all-Optional struct fields always
//! matches the **first** variant (Ollama), silently absorbing the `provider:` key and discarding it.
//! This is Pitfall 1 from PATTERNS.md — confirmed at Plan 02 execution time.
//!
//! **Chosen fallback (documented ADR):** `ProviderConfig.extra_request_options` and
//! `ProviderModelConfig.extra_request_options` use `Option<HashMap<String, serde_json::Value>>`
//! directly, which serde_yaml and serde_json handle correctly regardless of key names. The typed
//! structs (`OllamaExtraOptions`, `VllmExtraOptions`, `OpenRouterExtraOptions`, `ProviderRouting`,
//! `ProviderExtraOptions`) are retained for:
//! - Plan 03 schema generation (`hermes config show` schema keys)
//! - Future use as a validation/documentation layer
//! - Tests that verify the struct shapes
//!
//! The `resolve_extras` function is simpler under this fallback: no `serde_json::to_value` step
//! is needed on the deserialization side — the values are already `serde_json::Value`.
//!
//! # Design (D-01 fallback, D-03, D-04 — Phase 36.15)
//!
//! - [`OllamaExtraOptions`] — typed struct for schema docs / Plan 03 (not used for deserialization)
//! - [`VllmExtraOptions`] — typed struct for schema docs / Plan 03
//! - [`OpenRouterExtraOptions`] — typed struct for schema docs / Plan 03
//! - [`ProviderRouting`] — nested routing config for OpenRouter
//! - [`ProviderExtraOptions`] — untagged enum retained for Plan 03 schema generation only
//! - [`ProviderModelConfig`] — per-model config wrapper using raw HashMap for extras
//! - [`resolve_extras`] — merge helper returning `HashMap<String, serde_json::Value>`

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

// =============================================================================
// OllamaExtraOptions
// =============================================================================

/// Per-request options forwarded into the Ollama request body.
///
/// All fields map 1-to-1 to Ollama API parameters. `None` fields are omitted
/// from the serialized JSON (no null keys on the wire — T-36.15-06).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OllamaExtraOptions {
    /// Ollama context window size in tokens (`num_ctx`).
    /// Set this to avoid `exceed_context_size_error` for large prompts (D-08).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_ctx: Option<u32>,

    /// Max tokens to predict (`num_predict`). Negative values have provider-specific semantics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<i32>,

    /// Top-k sampling (`top_k`). Reduces probability of generating nonsense.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,

    /// Top-p nucleus sampling (`top_p`). Works with top-k.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,

    /// Number of tokens to process in parallel (`num_batch`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_batch: Option<u32>,
}

// =============================================================================
// VllmExtraOptions
// =============================================================================

/// Per-request options forwarded into the vLLM request body.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VllmExtraOptions {
    /// Top-k sampling limit (`top_k`). `-1` means disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i32>,

    /// Top-p nucleus sampling (`top_p`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,

    /// Repetition penalty — values > 1.0 discourage repetition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repetition_penalty: Option<f64>,

    /// Min-p sampling threshold.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f64>,
}

// =============================================================================
// OpenRouterExtraOptions + ProviderRouting
// =============================================================================

/// Provider routing preferences for OpenRouter requests.
///
/// Serializes as a nested JSON object under the `provider` key in the request body.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderRouting {
    /// Preferred provider order (e.g., `["anthropic", "openai"]`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,

    /// Providers to skip entirely.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore: Option<Vec<String>>,

    /// Preferred quantization levels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantizations: Option<Vec<String>>,
}

/// Per-request options forwarded into the OpenRouter request body.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenRouterExtraOptions {
    /// OpenRouter provider routing preferences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderRouting>,
}

// =============================================================================
// ProviderExtraOptions (untagged enum)
// =============================================================================

/// Discriminated union of per-provider extra request options.
///
/// Uses `#[serde(untagged)]` — serde tries each variant in declaration order.
/// Declaration order is load-bearing (PATTERNS.md Pitfall 1):
/// 1. `Ollama` first — `num_ctx` is unique to Ollama structs
/// 2. `OpenRouter` second — `provider:` key is unique to OpenRouter
/// 3. `Vllm` last — its keys (`top_k`, `top_p`) overlap with others
///
/// During serde_yaml deserialization, the first variant whose fields successfully
/// absorb all keys without error wins. Because every field is `Option<T>`, an
/// all-None Ollama would match anything — which is why ordering matters and why
/// the Wave 0 canary tests validate this contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProviderExtraOptions {
    /// Ollama-specific extra options (e.g., `num_ctx`, `num_predict`).
    Ollama(OllamaExtraOptions),
    /// OpenRouter-specific extra options (e.g., nested `provider.order`).
    OpenRouter(OpenRouterExtraOptions),
    /// vLLM-specific extra options (e.g., `top_k`, `top_p`).
    Vllm(VllmExtraOptions),
}

// =============================================================================
// ProviderModelConfig
// =============================================================================

/// Per-model configuration override. Lives under `providers.<name>.models.<model>`.
///
/// Currently holds only `extra_request_options` for per-key override of the
/// provider-level extras (D-03). More per-model fields may be added in future plans.
///
/// # D-01 Deviation
/// `extra_request_options` uses `HashMap<String, serde_json::Value>` rather than
/// `Option<ProviderExtraOptions>` to avoid serde_yaml untagged enum ambiguity (Pitfall 1).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderModelConfig {
    /// Per-model override for extra request options (raw key-value map).
    /// When present, per-key merge with provider-level extras — per-model wins (D-03).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra_request_options: HashMap<String, serde_json::Value>,
}

// =============================================================================
// resolve_extras — merge helper
// =============================================================================

/// Resolve the merged extra request options for a (provider, model) pair.
///
/// # Merge semantics (D-03)
///
/// 1. Collect provider-level extras from `providers[provider_name].extra_request_options`.
/// 2. Collect per-model extras from `providers[provider_name].models[model_name].extra_request_options`.
/// 3. Merge: start from provider map, insert each per-model key (overwriting provider value).
///    Result: per-model keys win; provider keys fill gaps.
///
/// Under the D-01 fallback, both `extra_request_options` fields are already
/// `HashMap<String, serde_json::Value>` — no `serde_json::to_value` conversion needed.
///
/// # Returns
///
/// Empty `HashMap` if the provider is not found, has no extras, and has no models.
pub fn resolve_extras(
    providers: &HashMap<String, crate::config::ProviderConfig>,
    provider_name: &str,
    model_name: &str,
) -> HashMap<String, serde_json::Value> {
    let provider_cfg = providers.get(provider_name);

    // Provider-level extras are already a HashMap<String, serde_json::Value> (D-01 fallback).
    let provider_extras: HashMap<String, serde_json::Value> = provider_cfg
        .map(|p| p.extra_request_options.clone())
        .unwrap_or_default();

    // Per-model extras are also already a HashMap<String, serde_json::Value>.
    let model_extras: HashMap<String, serde_json::Value> = provider_cfg
        .and_then(|p| p.models.get(model_name))
        .map(|m| m.extra_request_options.clone())
        .unwrap_or_default();

    // D-03: per-model wins per-key (insert overwrites).
    let mut merged = provider_extras;
    for (k, v) in model_extras {
        merged.insert(k, v);
    }
    merged
}

// =============================================================================
// Unit tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;

    /// Helper: build a ProviderConfig with Ollama extras as a raw HashMap (D-01 fallback).
    fn ollama_provider(num_ctx: Option<u32>, num_predict: Option<i32>) -> ProviderConfig {
        let mut extras = HashMap::new();
        if let Some(v) = num_ctx {
            extras.insert("num_ctx".to_string(), serde_json::json!(v));
        }
        if let Some(v) = num_predict {
            extras.insert("num_predict".to_string(), serde_json::json!(v));
        }
        ProviderConfig {
            extra_request_options: extras,
            ..Default::default()
        }
    }

    /// Test 1: empty providers map → empty HashMap.
    #[test]
    fn resolve_extras_returns_empty_when_no_extras() {
        let map: HashMap<String, ProviderConfig> = HashMap::new();
        let result = resolve_extras(&map, "ollama", "any");
        assert!(result.is_empty(), "empty providers must return empty map");
    }

    /// Test 2: provider has num_ctx=8192 but no per-model entry.
    /// resolve_extras for any model must return the provider default.
    #[test]
    fn resolve_extras_provider_only_falls_back_to_provider_default() {
        let mut map = HashMap::new();
        map.insert("ollama".to_string(), ollama_provider(Some(8192), None));
        let result = resolve_extras(&map, "ollama", "llama3.1:70b");
        assert_eq!(
            result.get("num_ctx"),
            Some(&serde_json::json!(8192)),
            "provider-level num_ctx must appear when no per-model entry exists"
        );
        assert!(!result.contains_key("num_predict"), "unset num_predict must not appear");
    }

    /// Test 3: per-model key wins over provider key (D-03 shallow merge).
    /// Provider: num_ctx=8192, num_predict=128.
    /// Model llama3.1:8b: num_ctx=32768 (no num_predict).
    /// Expected result for llama3.1:8b: num_ctx=32768, num_predict=128 (provider fill-in).
    #[test]
    fn resolve_extras_per_model_wins_per_key_d03() {
        let mut map = HashMap::new();
        let mut provider = ollama_provider(Some(8192), Some(128));
        // Add per-model entry for llama3.1:8b with num_ctx=32768 only.
        let mut model_extras = HashMap::new();
        model_extras.insert("num_ctx".to_string(), serde_json::json!(32768u32));
        provider.models.insert(
            "llama3.1:8b".to_string(),
            ProviderModelConfig {
                extra_request_options: model_extras,
            },
        );
        map.insert("ollama".to_string(), provider);

        let result = resolve_extras(&map, "ollama", "llama3.1:8b");
        assert_eq!(
            result.get("num_ctx"),
            Some(&serde_json::json!(32768u32)),
            "per-model num_ctx must win (32768, not 8192)"
        );
        assert_eq!(
            result.get("num_predict"),
            Some(&serde_json::json!(128i32)),
            "num_predict absent from per-model → must fall back to provider value (128)"
        );
    }

    /// Test 4: OpenRouter nested provider.order survives HashMap round-trip.
    /// Under the D-01 fallback, the nested object is a serde_json::Value::Object.
    #[test]
    fn resolve_extras_openrouter_provider_order_nested_preserved() {
        let mut extras = HashMap::new();
        extras.insert(
            "provider".to_string(),
            serde_json::json!({"order": ["anthropic", "openai"]}),
        );
        let mut map = HashMap::new();
        map.insert(
            "openrouter".to_string(),
            ProviderConfig {
                extra_request_options: extras,
                ..Default::default()
            },
        );

        let result = resolve_extras(&map, "openrouter", "any-model");
        let provider_val = result.get("provider").expect("provider key must be present");
        let order = provider_val
            .as_object()
            .and_then(|o| o.get("order"))
            .and_then(|v| v.as_array())
            .expect("provider.order must be a JSON array");
        assert_eq!(
            order,
            &vec![
                serde_json::Value::String("anthropic".to_string()),
                serde_json::Value::String("openai".to_string()),
            ],
            "OpenRouter provider.order must survive HashMap round-trip"
        );
        // Verify that ignore and quantizations (unset) do NOT appear as null keys (T-36.15-06).
        let provider_obj = provider_val.as_object().unwrap();
        assert!(
            !provider_obj.contains_key("ignore"),
            "unset ignore must not appear in serialized output (T-36.15-06)"
        );
        assert!(
            !provider_obj.contains_key("quantizations"),
            "unset quantizations must not appear in serialized output (T-36.15-06)"
        );
    }

    /// Test 5: unknown provider name returns empty HashMap (no panic).
    #[test]
    fn resolve_extras_unknown_provider_returns_empty() {
        let mut map = HashMap::new();
        map.insert("ollama".to_string(), ollama_provider(Some(8192), None));
        let result = resolve_extras(&map, "nonexistent_provider", "any_model");
        assert!(result.is_empty(), "unknown provider must return empty map, not panic");
    }

    /// Test 6 (compile-only): all four typed structs implement Default.
    /// OllamaExtraOptions::default() must produce all-None fields.
    /// These structs are kept for Plan 03 schema generation; their Default impls are exercised here.
    #[test]
    fn provider_extra_options_default_is_none_friendly() {
        let ollama = OllamaExtraOptions::default();
        assert!(ollama.num_ctx.is_none());
        assert!(ollama.num_predict.is_none());
        assert!(ollama.top_k.is_none());
        assert!(ollama.top_p.is_none());
        assert!(ollama.num_batch.is_none());

        let vllm = VllmExtraOptions::default();
        assert!(vllm.top_k.is_none());
        assert!(vllm.top_p.is_none());
        assert!(vllm.repetition_penalty.is_none());
        assert!(vllm.min_p.is_none());

        let openrouter = OpenRouterExtraOptions::default();
        assert!(openrouter.provider.is_none());

        let routing = ProviderRouting::default();
        assert!(routing.order.is_none());
        assert!(routing.ignore.is_none());
        assert!(routing.quantizations.is_none());

        // ProviderModelConfig::default() must have empty extras map.
        let model_cfg = ProviderModelConfig::default();
        assert!(model_cfg.extra_request_options.is_empty());
    }
}

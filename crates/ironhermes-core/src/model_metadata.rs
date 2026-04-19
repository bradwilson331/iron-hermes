// Model metadata types and registry for model-aware context lengths and capabilities.
// Phase 21.3 — replaces hardcoded DEFAULT_CONTEXT_LENGTH with model-driven metadata.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::constants::DEFAULT_CONTEXT_LENGTH;

/// Capabilities a model supports (per D-10).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub vision: bool,
    pub tool_use: bool,
    pub reasoning: bool,
    pub streaming: bool,
}

/// Metadata for a single model (per D-10).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetadata {
    pub context_length: usize,
    pub max_output_tokens: Option<usize>,
    /// tiktoken encoding name: "cl100k_base" or "o200k_base"
    pub tokenizer: String,
    pub capabilities: ModelCapabilities,
}

/// Registry of model metadata with static table, alias map, and disk cache overlay (per D-01, D-05, D-06).
pub struct ModelRegistry {
    table: HashMap<&'static str, ModelMetadata>,
    aliases: HashMap<&'static str, &'static str>,
    cache: HashMap<String, ModelMetadata>,
}

impl ModelRegistry {
    /// Create a new registry with the built-in static table and alias map.
    pub fn new() -> Self {
        Self {
            table: build_static_table(),
            aliases: build_alias_map(),
            cache: HashMap::new(),
        }
    }

    /// Look up model metadata by model ID.
    ///
    /// Resolution chain (per D-05):
    /// 1. Exact match in cache
    /// 2. Exact match in static table
    /// 3. Alias map -> canonical -> try cache then static table
    /// 4. Strip `provider/` prefix -> try cache, static table, alias map
    /// 5. Return None
    pub fn lookup(&self, model_id: &str) -> Option<&ModelMetadata> {
        // 1. Exact match in cache
        if let Some(m) = self.cache.get(model_id) {
            return Some(m);
        }
        // 2. Exact match in static table
        if let Some(m) = self.table.get(model_id) {
            return Some(m);
        }
        // 3. Alias map -> canonical -> try cache then static table
        if let Some(&canonical) = self.aliases.get(model_id) {
            if let Some(m) = self.cache.get(canonical) {
                return Some(m);
            }
            if let Some(m) = self.table.get(canonical) {
                return Some(m);
            }
        }
        // 4. Strip provider/ prefix -> try cache, static table, alias map
        if let Some((_provider, bare)) = model_id.split_once('/') {
            if let Some(m) = self.cache.get(bare) {
                return Some(m);
            }
            if let Some(m) = self.table.get(bare) {
                return Some(m);
            }
            if let Some(&canonical) = self.aliases.get(bare) {
                if let Some(m) = self.cache.get(canonical) {
                    return Some(m);
                }
                if let Some(m) = self.table.get(canonical) {
                    return Some(m);
                }
            }
        }
        // 5. Not found
        None
    }

    /// Returns the context length for a model, or DEFAULT_CONTEXT_LENGTH if unknown.
    /// Logs a tracing::warn! for unknown models (per D-15).
    pub fn context_length_or_default(&self, model_id: &str) -> usize {
        match self.lookup(model_id) {
            Some(m) => m.context_length,
            None => {
                tracing::warn!(
                    model_id,
                    default = DEFAULT_CONTEXT_LENGTH,
                    "unknown model, using default context length"
                );
                DEFAULT_CONTEXT_LENGTH
            }
        }
    }

    /// Merge disk cache entries into the registry.
    /// Cache entries override static table entries for the same key (per D-06).
    pub fn merge_cache(&mut self, entries: HashMap<String, ModelMetadata>) {
        self.cache.extend(entries);
    }

    /// Returns all entries (static + cache) sorted by canonical ID.
    /// Cache entries override static table entries for the same key.
    pub fn all_models(&self) -> Vec<(&str, &ModelMetadata)> {
        let mut result: HashMap<&str, &ModelMetadata> = HashMap::new();

        // Add static table entries
        for (&id, meta) in &self.table {
            result.insert(id, meta);
        }

        // Cache entries override static entries
        for (id, meta) in &self.cache {
            result.insert(id.as_str(), meta);
        }

        let mut sorted: Vec<(&str, &ModelMetadata)> = result.into_iter().collect();
        sorted.sort_by_key(|(id, _)| *id);
        sorted
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to build a ModelMetadata with cl100k_base tokenizer.
fn cl100k(
    context_length: usize,
    max_output_tokens: Option<usize>,
    vision: bool,
    tool_use: bool,
    reasoning: bool,
    streaming: bool,
) -> ModelMetadata {
    ModelMetadata {
        context_length,
        max_output_tokens,
        tokenizer: "cl100k_base".to_string(),
        capabilities: ModelCapabilities {
            vision,
            tool_use,
            reasoning,
            streaming,
        },
    }
}

/// Helper to build a ModelMetadata with o200k_base tokenizer.
fn o200k(
    context_length: usize,
    max_output_tokens: Option<usize>,
    vision: bool,
    tool_use: bool,
    reasoning: bool,
    streaming: bool,
) -> ModelMetadata {
    ModelMetadata {
        context_length,
        max_output_tokens,
        tokenizer: "o200k_base".to_string(),
        capabilities: ModelCapabilities {
            vision,
            tool_use,
            reasoning,
            streaming,
        },
    }
}

/// Build the static lookup table with known models (per D-01, CONTEXT.md Specifics).
fn build_static_table() -> HashMap<&'static str, ModelMetadata> {
    let mut m = HashMap::with_capacity(40);

    // ── Claude family (cl100k_base approximation) ──────────────────────
    // Claude 3.5 series
    m.insert("claude-haiku-3.5", cl100k(200_000, Some(8_192), true, true, false, true));
    m.insert("claude-sonnet-3.5", cl100k(200_000, Some(8_192), true, true, false, true));

    // Claude 4 series
    m.insert("claude-haiku-4", cl100k(200_000, Some(64_000), true, true, false, true));
    m.insert("claude-sonnet-4", cl100k(200_000, Some(64_000), true, true, false, true));
    m.insert("claude-opus-4", cl100k(200_000, Some(32_000), true, true, true, true));

    // Claude 4.5 series
    m.insert("claude-sonnet-4.5", cl100k(200_000, Some(64_000), true, true, false, true));

    // Claude 4.6 series
    m.insert("claude-opus-4.6", cl100k(1_000_000, Some(32_000), true, true, true, true));

    // ── GPT family (o200k_base for 4o+, cl100k_base for older) ────────
    m.insert("gpt-4o", o200k(128_000, Some(16_384), true, true, false, true));
    m.insert("gpt-4o-mini", o200k(128_000, Some(16_384), true, true, false, true));
    m.insert("gpt-4.1", o200k(1_047_576, Some(32_768), true, true, false, true));
    m.insert("gpt-4.1-mini", o200k(1_047_576, Some(32_768), true, true, false, true));
    m.insert("gpt-4.1-nano", o200k(1_047_576, Some(32_768), false, true, false, true));
    m.insert("o3", o200k(200_000, Some(100_000), true, true, true, true));
    m.insert("o3-mini", o200k(200_000, Some(100_000), false, true, true, true));
    m.insert("o4-mini", o200k(200_000, Some(100_000), true, true, true, true));

    // ── Llama family (cl100k_base approximation) ──────────────────────
    m.insert("llama-3.1-8b", cl100k(128_000, Some(4_096), false, true, false, true));
    m.insert("llama-3.1-70b", cl100k(128_000, Some(4_096), false, true, false, true));
    m.insert("llama-3.1-405b", cl100k(128_000, Some(4_096), false, true, false, true));
    m.insert("llama-3.3-70b", cl100k(128_000, Some(4_096), false, true, false, true));
    m.insert("llama-4-scout", cl100k(512_000, Some(16_384), true, true, false, true));
    m.insert("llama-4-maverick", cl100k(1_048_576, Some(16_384), true, true, false, true));

    // ── Gemini family (cl100k_base approximation) ─────────────────────
    m.insert("gemini-2.0-flash", cl100k(1_048_576, Some(8_192), true, true, false, true));
    m.insert("gemini-2.5-pro", cl100k(1_048_576, Some(65_536), true, true, true, true));
    m.insert("gemini-2.5-flash", cl100k(1_048_576, Some(65_536), true, true, true, true));

    // ── Mistral / Mixtral family (cl100k_base approximation) ──────────
    m.insert("mistral-large", cl100k(128_000, Some(4_096), false, true, false, true));
    m.insert("mistral-small", cl100k(128_000, Some(4_096), false, true, false, true));
    m.insert("mistral-medium", cl100k(32_000, Some(4_096), false, true, false, true));
    m.insert("mixtral-8x22b", cl100k(65_536, Some(4_096), false, true, false, true));
    m.insert("mixtral-8x7b", cl100k(32_768, Some(4_096), false, true, false, true));
    m.insert("codestral", cl100k(256_000, Some(8_192), false, true, false, true));

    // ── DeepSeek family (cl100k_base approximation) ───────────────────
    m.insert("deepseek-v3", cl100k(128_000, Some(8_192), false, true, false, true));
    m.insert("deepseek-r1", cl100k(164_000, Some(8_192), false, true, true, true));

    // ── Qwen family (cl100k_base approximation) ──────────────────────
    m.insert("qwen-2.5-72b", cl100k(128_000, Some(8_192), false, true, false, true));
    m.insert("qwen-2.5-coder-32b", cl100k(32_768, Some(8_192), false, true, false, true));
    m.insert("qwen-2.5-14b", cl100k(128_000, Some(8_192), false, true, false, true));
    m.insert("qwen-2.5-7b", cl100k(128_000, Some(8_192), false, true, false, true));

    m
}

/// Build the alias map for versioned, legacy, and provider-prefixed model names (per D-05).
fn build_alias_map() -> HashMap<&'static str, &'static str> {
    let mut a = HashMap::with_capacity(40);

    // ── Claude versioned aliases ──────────────────────────────────────
    a.insert("claude-haiku-3.5-20241022", "claude-haiku-3.5");
    a.insert("claude-sonnet-3.5-20241022", "claude-sonnet-3.5");
    a.insert("claude-haiku-4-20250414", "claude-haiku-4");
    a.insert("claude-sonnet-4-20250514", "claude-sonnet-4");
    a.insert("claude-opus-4-20250514", "claude-opus-4");
    a.insert("claude-sonnet-4.5-20250514", "claude-sonnet-4.5");

    // ── Claude legacy aliases ─────────────────────────────────────────
    a.insert("claude-3-5-haiku-latest", "claude-haiku-3.5");
    a.insert("claude-3-5-haiku-20241022", "claude-haiku-3.5");
    a.insert("claude-3-5-sonnet-latest", "claude-sonnet-3.5");
    a.insert("claude-3-5-sonnet-20241022", "claude-sonnet-3.5");
    a.insert("claude-3-5-sonnet-v2", "claude-sonnet-3.5");
    a.insert("claude-sonnet-4-0", "claude-sonnet-4");
    a.insert("claude-opus-4-0", "claude-opus-4");

    // ── GPT versioned/legacy aliases ──────────────────────────────────
    a.insert("gpt-4o-2024-11-20", "gpt-4o");
    a.insert("gpt-4o-2024-08-06", "gpt-4o");
    a.insert("gpt-4o-2024-05-13", "gpt-4o");
    a.insert("gpt-4o-mini-2024-07-18", "gpt-4o-mini");
    a.insert("gpt-4-turbo", "gpt-4o");
    a.insert("gpt-4-turbo-preview", "gpt-4o");
    a.insert("chatgpt-4o-latest", "gpt-4o");

    // ── Llama aliases ────────────────────────────────────────────────
    a.insert("meta-llama/llama-3.1-8b-instruct", "llama-3.1-8b");
    a.insert("meta-llama/llama-3.1-70b-instruct", "llama-3.1-70b");
    a.insert("meta-llama/llama-3.1-405b-instruct", "llama-3.1-405b");
    a.insert("meta-llama/llama-3.3-70b-instruct", "llama-3.3-70b");

    // ── Gemini aliases ───────────────────────────────────────────────
    a.insert("gemini-2.0-flash-001", "gemini-2.0-flash");
    a.insert("gemini-2.5-pro-preview", "gemini-2.5-pro");
    a.insert("gemini-2.5-flash-preview", "gemini-2.5-flash");

    // ── Mistral aliases ──────────────────────────────────────────────
    a.insert("mistral-large-latest", "mistral-large");
    a.insert("mistral-small-latest", "mistral-small");
    a.insert("mistral-medium-latest", "mistral-medium");

    // ── DeepSeek aliases ─────────────────────────────────────────────
    a.insert("deepseek-chat", "deepseek-v3");
    a.insert("deepseek-reasoner", "deepseek-r1");

    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_claude_sonnet_4() {
        let reg = ModelRegistry::new();
        let meta = reg.lookup("claude-sonnet-4").expect("claude-sonnet-4 should exist");
        assert_eq!(meta.context_length, 200_000);
        assert_eq!(meta.max_output_tokens, Some(64_000));
        assert_eq!(meta.tokenizer, "cl100k_base");
        assert!(meta.capabilities.vision);
        assert!(meta.capabilities.tool_use);
    }

    #[test]
    fn lookup_claude_opus_4() {
        let reg = ModelRegistry::new();
        let meta = reg.lookup("claude-opus-4").expect("claude-opus-4 should exist");
        assert_eq!(meta.context_length, 200_000);
        assert!(meta.capabilities.reasoning);
    }

    #[test]
    fn lookup_gpt_4o() {
        let reg = ModelRegistry::new();
        let meta = reg.lookup("gpt-4o").expect("gpt-4o should exist");
        assert_eq!(meta.tokenizer, "o200k_base");
        assert_eq!(meta.context_length, 128_000);
    }

    #[test]
    fn lookup_provider_prefix_stripping() {
        let reg = ModelRegistry::new();
        let meta = reg.lookup("anthropic/claude-sonnet-4").expect("provider prefix should resolve");
        assert_eq!(meta.context_length, 200_000);
    }

    #[test]
    fn lookup_versioned_alias() {
        let reg = ModelRegistry::new();
        let meta = reg.lookup("claude-sonnet-4-20250514").expect("versioned alias should resolve");
        assert_eq!(meta.context_length, 200_000);
    }

    #[test]
    fn lookup_openai_prefix_stripping() {
        let reg = ModelRegistry::new();
        let meta = reg.lookup("openai/gpt-4o").expect("openai prefix should resolve");
        assert_eq!(meta.tokenizer, "o200k_base");
    }

    #[test]
    fn lookup_nonexistent_returns_none() {
        let reg = ModelRegistry::new();
        assert!(reg.lookup("nonexistent-model-xyz").is_none());
    }

    #[test]
    fn merge_cache_overrides_static_table() {
        let mut reg = ModelRegistry::new();
        let mut entries = HashMap::new();
        entries.insert(
            "claude-sonnet-4".to_string(),
            ModelMetadata {
                context_length: 999_999,
                max_output_tokens: Some(100_000),
                tokenizer: "cl100k_base".to_string(),
                capabilities: ModelCapabilities::default(),
            },
        );
        reg.merge_cache(entries);
        let meta = reg.lookup("claude-sonnet-4").expect("should find cached entry");
        assert_eq!(meta.context_length, 999_999);
    }

    #[test]
    fn context_length_or_default_known_model() {
        let reg = ModelRegistry::new();
        assert_eq!(reg.context_length_or_default("claude-sonnet-4"), 200_000);
    }

    #[test]
    fn context_length_or_default_unknown_model() {
        let reg = ModelRegistry::new();
        assert_eq!(
            reg.context_length_or_default("unknown-model"),
            DEFAULT_CONTEXT_LENGTH
        );
    }

    #[test]
    fn static_table_has_at_least_30_models() {
        let reg = ModelRegistry::new();
        let all = reg.all_models();
        assert!(
            all.len() >= 30,
            "expected at least 30 models, got {}",
            all.len()
        );
    }

    #[test]
    fn all_models_sorted_by_id() {
        let reg = ModelRegistry::new();
        let all = reg.all_models();
        let ids: Vec<&str> = all.iter().map(|(id, _)| *id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(
            ids, sorted,
            "all_models should return entries sorted by canonical ID"
        );
    }
}

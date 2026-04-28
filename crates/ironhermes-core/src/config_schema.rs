//! Config schema types for memory (and future) provider plugins (D-06).
//!
//! `ConfigField` mirrors the hermes-agent plugin contract 1:1 so providers
//! can describe their own configuration surface to setup wizards.
//! `MemoryAction` is the small enum fired via `MemoryProvider::on_memory_write`
//! so a mirror subscriber can observe primary writes without owning the
//! dispatch semantics.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigField {
    pub key: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub cache_breaking: bool,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub choices: Option<Vec<String>>,
    #[serde(default)]
    pub env_var: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryAction {
    Add,
    Replace,
    Remove,
}

/// Returns the full static SCHEMA registry of all config fields
/// tagged with their `secret` and `cache_breaking` disposition
/// per D-09 (secret fields) and D-13/D-18 (cache-breaking fields).
///
/// Using a function rather than a `static` avoids heap-allocation
/// const-init issues with `String` keys.
pub fn schema() -> Vec<ConfigField> {
    vec![
        ConfigField {
            key: "model.default".into(),
            description: Some("Default LLM model identifier".into()),
            secret: false,
            required: true,
            cache_breaking: true,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "model.base_url".into(),
            description: Some("Override provider base URL".into()),
            secret: false,
            required: false,
            cache_breaking: true,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "model.api_key".into(),
            description: Some("API key for default provider".into()),
            secret: true,
            required: true,
            cache_breaking: true, // D-13 exception: api_key is both secret AND cache_breaking
            default: None,
            choices: None,
            env_var: Some("OPENAI_API_KEY".into()),
            url: None,
        },
        ConfigField {
            key: "model.provider".into(),
            description: Some("Provider name (openrouter, anthropic, ...)".into()),
            secret: false,
            required: true,
            cache_breaking: false,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "agent.system_prompt".into(),
            description: Some("Custom system prompt prefix (cache-breaking)".into()),
            secret: false,
            required: false,
            cache_breaking: true,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "agent.personality".into(),
            description: Some("Personality preset (slot 1 of 10-layer prompt)".into()),
            secret: false,
            required: false,
            cache_breaking: true,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "memory.provider".into(),
            description: Some("Memory backend: file/sqlite/grafeo/duckdb".into()),
            secret: false,
            required: false,
            cache_breaking: true,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "memory.memory_enabled".into(),
            description: Some("Enable MEMORY.md layer (Learning Loop component)".into()),
            secret: false,
            required: false,
            cache_breaking: true,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "memory.user_profile_enabled".into(),
            description: Some("Enable USER.md layer".into()),
            secret: false,
            required: false,
            cache_breaking: false,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "learning.skill_generation_enabled".into(),
            description: Some("Autonomous skill creation (Phase 33 reservation)".into()),
            secret: false,
            required: false,
            cache_breaking: true,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "learning.periodic_nudge_interval_seconds".into(),
            description: Some("Periodic nudge interval (Phase 32 reservation)".into()),
            secret: false,
            required: false,
            cache_breaking: false,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "learning.reflection_depth".into(),
            description: Some("Reflection depth: light|standard|deep (Phase 33)".into()),
            secret: false,
            required: false,
            cache_breaking: false,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "learning.skill_eval".into(),
            description: Some("Validate skills before adding to library (Phase 33)".into()),
            secret: false,
            required: false,
            cache_breaking: false,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "learning.max_skills".into(),
            description: Some("Cap before pruning low-use skills (Phase 33)".into()),
            secret: false,
            required: false,
            cache_breaking: false,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "gateway.platforms.telegram.token".into(),
            description: Some("Telegram bot token".into()),
            secret: true,
            required: false,
            cache_breaking: false,
            default: None,
            choices: None,
            env_var: Some("TELEGRAM_BOT_TOKEN".into()),
            url: None,
        },
        ConfigField {
            key: "gateway.platforms.telegram.api_key".into(),
            description: Some("Telegram API key (alt auth)".into()),
            secret: true,
            required: false,
            cache_breaking: false,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "subagent.api_key".into(),
            description: Some("Subagent provider API key".into()),
            secret: true,
            required: false,
            cache_breaking: false,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
        ConfigField {
            key: "batch.api_key".into(),
            description: Some("Batch processing API key".into()),
            secret: true,
            required: false,
            cache_breaking: false,
            default: None,
            choices: None,
            env_var: None,
            url: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_field_roundtrip_all_fields() {
        let f = ConfigField {
            key: "API_KEY".into(),
            description: Some("Service key".into()),
            secret: true,
            required: true,
            cache_breaking: true,
            default: Some(serde_json::json!("sk-...")),
            choices: Some(vec!["a".into(), "b".into()]),
            env_var: Some("MY_API_KEY".into()),
            url: Some("https://example.com".into()),
        };
        let s = serde_json::to_string(&f).unwrap();
        let back: ConfigField = serde_json::from_str(&s).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn memory_action_lowercase_serde() {
        assert_eq!(serde_json::to_string(&MemoryAction::Add).unwrap(), "\"add\"");
        assert_eq!(serde_json::to_string(&MemoryAction::Replace).unwrap(), "\"replace\"");
        assert_eq!(serde_json::to_string(&MemoryAction::Remove).unwrap(), "\"remove\"");
        let a: MemoryAction = serde_json::from_str("\"add\"").unwrap();
        assert_eq!(a, MemoryAction::Add);
    }

    #[test]
    fn schema_contains_all_cache_breaking_fields() {
        let s = schema();
        let cache_breaking_keys: Vec<&str> = s.iter()
            .filter(|f| f.cache_breaking)
            .map(|f| f.key.as_str())
            .collect();

        // D-13 + D-18 cache-breaking fields
        let expected = [
            "model.default",
            "model.base_url",
            "model.api_key",
            "agent.system_prompt",
            "agent.personality",
            "memory.provider",
            "memory.memory_enabled",
            "learning.skill_generation_enabled",
        ];
        for key in &expected {
            assert!(
                cache_breaking_keys.contains(key),
                "schema missing cache_breaking entry for: {key}"
            );
        }
    }

    #[test]
    fn schema_contains_all_secret_fields() {
        let s = schema();
        let secret_keys: Vec<&str> = s.iter()
            .filter(|f| f.secret)
            .map(|f| f.key.as_str())
            .collect();

        // D-09 secret fields
        let expected = [
            "model.api_key",
            "gateway.platforms.telegram.token",
            "gateway.platforms.telegram.api_key",
            "subagent.api_key",
            "batch.api_key",
        ];
        for key in &expected {
            assert!(
                secret_keys.contains(key),
                "schema missing secret entry for: {key}"
            );
        }
    }

    #[test]
    fn model_api_key_is_both_secret_and_cache_breaking() {
        let s = schema();
        let api_key = s.iter().find(|f| f.key == "model.api_key")
            .expect("model.api_key must be in schema");
        assert!(api_key.secret, "model.api_key must be secret");
        assert!(api_key.cache_breaking, "model.api_key must be cache_breaking (D-13 exception)");

        // All other secret fields must NOT be cache_breaking
        for f in s.iter().filter(|f| f.secret && f.key != "model.api_key") {
            assert!(
                !f.cache_breaking,
                "field {} is secret but should NOT be cache_breaking (only model.api_key is the D-13 exception)",
                f.key
            );
        }
    }
}

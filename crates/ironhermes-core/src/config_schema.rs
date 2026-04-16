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
}

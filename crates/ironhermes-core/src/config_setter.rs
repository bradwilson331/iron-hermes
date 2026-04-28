//! `config_setter.rs` — dotted-path get/set over config.yaml using
//! `serde_yaml::Value`. Required because `Config::save()` round-trips
//! through Rust struct serialization and would drop unknown keys
//! (e.g., `learning.*` keys reserved for Phase 32/33 — see D-15).

use anyhow::{Context, Result};
use crate::config_schema::ConfigField;
use std::path::Path;

fn load_doc(cfg_path: &Path) -> Result<serde_yaml::Value> {
    if cfg_path.exists() {
        let text = std::fs::read_to_string(cfg_path)
            .with_context(|| format!("reading {}", cfg_path.display()))?;
        Ok(serde_yaml::from_str(&text)
            .unwrap_or(serde_yaml::Value::Mapping(Default::default())))
    } else {
        Ok(serde_yaml::Value::Mapping(Default::default()))
    }
}

fn save_doc(cfg_path: &Path, doc: &serde_yaml::Value) -> Result<()> {
    let text = serde_yaml::to_string(doc)?;
    std::fs::write(cfg_path, text)
        .with_context(|| format!("writing {}", cfg_path.display()))?;
    Ok(())
}

/// Walk `keys` into `doc`, creating intermediate Mappings as needed.
/// Sets the leaf to `leaf_value` and returns the previous leaf as a String (None if absent).
fn set_at(doc: &mut serde_yaml::Value, keys: &[&str], leaf_value: serde_yaml::Value) -> Result<Option<String>> {
    let mut node = doc;
    for (i, key) in keys.iter().enumerate() {
        let key_v = serde_yaml::Value::String((*key).to_string());
        // Ensure node is a Mapping.
        if !matches!(node, serde_yaml::Value::Mapping(_)) {
            *node = serde_yaml::Value::Mapping(Default::default());
        }
        let map = node.as_mapping_mut().unwrap();
        if i == keys.len() - 1 {
            let old = map.insert(key_v.clone(), leaf_value.clone());
            return Ok(old.map(|v| match v {
                serde_yaml::Value::String(s) => s,
                other => serde_yaml::to_string(&other).unwrap_or_default().trim().to_string(),
            }));
        }
        // Descend, creating an empty mapping if missing.
        if !map.contains_key(&key_v) {
            map.insert(key_v.clone(), serde_yaml::Value::Mapping(Default::default()));
        }
        node = map.get_mut(&key_v).unwrap();
    }
    Ok(None)
}

/// Walk `keys` into `doc` and return the leaf as a raw scalar String, or None.
fn get_at(doc: &serde_yaml::Value, keys: &[&str]) -> Option<String> {
    let mut node = doc;
    for key in keys {
        let key_v = serde_yaml::Value::String((*key).to_string());
        node = match node.as_mapping().and_then(|m| m.get(&key_v)) {
            Some(v) => v,
            None => return None,
        };
    }
    match node {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        other => Some(serde_yaml::to_string(other).unwrap_or_default().trim().to_string()),
    }
}

/// Set a config value at `dotted_path` in `hermes_home/config.yaml`.
/// Creates the file if it doesn't exist. Creates intermediate mappings as needed.
/// Returns the old value as a String if the key existed previously, or None if new.
pub fn config_set(hermes_home: &Path, dotted_path: &str, value: &str) -> Result<Option<String>> {
    let cfg_path = hermes_home.join("config.yaml");
    let mut doc = load_doc(&cfg_path)?;
    let keys: Vec<&str> = dotted_path.split('.').collect();
    // Coerce common scalar types: bool, integer, otherwise string.
    let leaf = if let Ok(b) = value.parse::<bool>() {
        serde_yaml::Value::Bool(b)
    } else if let Ok(n) = value.parse::<i64>() {
        serde_yaml::Value::Number(n.into())
    } else {
        serde_yaml::Value::String(value.to_string())
    };
    let old = set_at(&mut doc, &keys, leaf)?;
    save_doc(&cfg_path, &doc)?;
    Ok(old)
}

/// Get a config value at `dotted_path` as a raw scalar string.
/// Returns Ok(None) if the key doesn't exist or the file doesn't exist.
pub fn config_get(hermes_home: &Path, dotted_path: &str) -> Result<Option<String>> {
    let cfg_path = hermes_home.join("config.yaml");
    if !cfg_path.exists() { return Ok(None); }
    let doc = load_doc(&cfg_path)?;
    let keys: Vec<&str> = dotted_path.split('.').collect();
    Ok(get_at(&doc, &keys))
}

/// Lookup whether `dotted_path` is tagged `cache_breaking: true` in the SCHEMA.
pub fn is_cache_breaking(dotted_path: &str, schema: &[ConfigField]) -> bool {
    schema.iter().any(|f| f.key == dotted_path && f.cache_breaking)
}

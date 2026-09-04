//! `config_setter.rs` — dotted-path get/set over config.yaml using
//! `serde_yaml::Value`. Required because `Config::save()` round-trips
//! through Rust struct serialization and would drop unknown keys
//! (e.g., `learning.*` keys reserved for Phase 32/33 — see D-15).

use crate::config_schema::ConfigField;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

fn load_doc(cfg_path: &Path) -> Result<serde_yaml::Value> {
    if cfg_path.exists() {
        let text = std::fs::read_to_string(cfg_path)
            .with_context(|| format!("reading {}", cfg_path.display()))?;
        Ok(serde_yaml::from_str(&text).unwrap_or(serde_yaml::Value::Mapping(Default::default())))
    } else {
        Ok(serde_yaml::Value::Mapping(Default::default()))
    }
}

fn save_doc(cfg_path: &Path, doc: &serde_yaml::Value) -> Result<()> {
    let text = serde_yaml::to_string(doc)?;
    std::fs::write(cfg_path, text).with_context(|| format!("writing {}", cfg_path.display()))?;
    Ok(())
}

/// Walk `keys` into `doc`, creating intermediate Mappings as needed.
/// Sets the leaf to `leaf_value` and returns the previous leaf as a String (None if absent).
fn set_at(
    doc: &mut serde_yaml::Value,
    keys: &[&str],
    leaf_value: serde_yaml::Value,
) -> Result<Option<String>> {
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
                other => serde_yaml::to_string(&other)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            }));
        }
        // Descend, creating an empty mapping if missing.
        if !map.contains_key(&key_v) {
            map.insert(
                key_v.clone(),
                serde_yaml::Value::Mapping(Default::default()),
            );
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
        node = node.as_mapping().and_then(|m| m.get(&key_v))?;
    }
    match node {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        other => Some(
            serde_yaml::to_string(other)
                .unwrap_or_default()
                .trim()
                .to_string(),
        ),
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
    if !cfg_path.exists() {
        return Ok(None);
    }
    let doc = load_doc(&cfg_path)?;
    let keys: Vec<&str> = dotted_path.split('.').collect();
    Ok(get_at(&doc, &keys))
}

/// Lookup whether `dotted_path` is tagged `cache_breaking: true` in the SCHEMA.
pub fn is_cache_breaking(dotted_path: &str, schema: &[ConfigField]) -> bool {
    schema
        .iter()
        .any(|f| f.key == dotted_path && f.cache_breaking)
}

/// Phase 49.4.1 (D-12): atomic variant of `save_doc` — copies
/// `Config::save_to`'s temp+rename discipline (`config.rs:3800-3809`)
/// rather than this module's own plain `std::fs::write`, which leaves a
/// torn-write window on crash/power-loss. Required by
/// `mirror_providers_subtree`: a partial `providers:` rewrite mid-sync must
/// never leave a profile's `config.yaml` half-written.
pub fn save_doc_atomic(cfg_path: &Path, doc: &serde_yaml::Value) -> Result<()> {
    if let Some(parent) = cfg_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create_dir_all {}", parent.display()))?;
    }
    let text = serde_yaml::to_string(doc)?;
    let tmp = cfg_path.with_extension("yaml.tmp");
    std::fs::write(&tmp, &text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, cfg_path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), cfg_path.display()))?;
    Ok(())
}

/// Phase 49.4.1 (D-02/D-03/D-12): mirror ONLY the connection fields
/// (`base_url`/`api_key_env`/`api_mode`/`disabled`) of each entry in
/// `root_connection_fields` onto the profile's own `providers:` subtree —
/// never a wholesale provider-entry replacement, which would clobber a
/// profile-local `default_model`/`fallback_providers` override (D-03:
/// root's `openrouter.default_model` is an opus model while a profile may
/// deliberately pin a haiku one, and the per-provider value overrides the
/// top-level `model.default`). Every other top-level key of the document,
/// and every OTHER key already present on a touched provider entry, is
/// passed through untouched (D-12) — this fn never round-trips the WHOLE
/// document through `Config`'s typed (de)serialization, only surgical
/// `Value` mutation, so unknown keys and sibling sections survive
/// (comments do not — `serde_yaml` has no comment-carrying value; this is
/// an accepted, stated cost, not a defect).
///
/// When `secrets_source` is `Some`, also sets the top-level `secrets_source`
/// key to that string (the persisted remembered-source vocabulary, D-01/D-05).
///
/// Returns the number of provider entries touched. Finishes with
/// [`save_doc_atomic`] (never the non-atomic `save_doc` above — a torn
/// write here corrupts the profile's provider registry).
pub fn mirror_providers_subtree(
    profile_cfg_path: &Path,
    root_connection_fields: &BTreeMap<String, serde_yaml::Mapping>,
    secrets_source: Option<&str>,
) -> Result<usize> {
    let mut doc = load_doc(profile_cfg_path)?;
    if !matches!(doc, serde_yaml::Value::Mapping(_)) {
        doc = serde_yaml::Value::Mapping(Default::default());
    }
    let root_map = doc
        .as_mapping_mut()
        .expect("doc coerced to Mapping immediately above");

    let providers_key = serde_yaml::Value::String("providers".to_string());
    if !matches!(root_map.get(&providers_key), Some(serde_yaml::Value::Mapping(_))) {
        root_map.insert(
            providers_key.clone(),
            serde_yaml::Value::Mapping(Default::default()),
        );
    }
    let providers_map = root_map
        .get_mut(&providers_key)
        .and_then(|v| v.as_mapping_mut())
        .expect("providers key coerced to Mapping immediately above");

    let mut touched = 0usize;
    for (name, fields) in root_connection_fields {
        let name_key = serde_yaml::Value::String(name.clone());
        if !matches!(providers_map.get(&name_key), Some(serde_yaml::Value::Mapping(_))) {
            providers_map.insert(
                name_key.clone(),
                serde_yaml::Value::Mapping(Default::default()),
            );
        }
        let entry_map = providers_map
            .get_mut(&name_key)
            .and_then(|v| v.as_mapping_mut())
            .expect("provider entry coerced to Mapping immediately above");
        for (field_key, field_value) in fields {
            entry_map.insert(field_key.clone(), field_value.clone());
        }
        touched += 1;
    }

    if let Some(source) = secrets_source {
        root_map.insert(
            serde_yaml::Value::String("secrets_source".to_string()),
            serde_yaml::Value::String(source.to_string()),
        );
    }

    save_doc_atomic(profile_cfg_path, &doc)?;
    Ok(touched)
}

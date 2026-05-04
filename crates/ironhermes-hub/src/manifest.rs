//! Phase 19.1 HubManifest — DEPRECATED in 21.8.
//!
//! Retained ONLY for one-way migration to `SkillLock` (see
//! `lock::migrate_from_hub_manifest`). `HubManifest::save()` is `#[deprecated]` —
//! no new call sites permitted. Read path (`HubManifest::load_or_default`) stays
//! functional for migration.
//!
//! Scheduled for deletion in phase 21.9 once 19.1 → 21.8 migration has settled.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct HubManifest {
    #[serde(default)]
    pub installed: HashMap<String, ManifestEntry>,
    #[serde(flatten, default)]
    pub extras: HashMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ManifestEntry {
    pub name: String,
    pub source: String,
    pub identifier: String,
    pub content_hash: String,
    pub scan_verdict: String,
    pub install_path: PathBuf,
    pub files: Vec<String>,
    pub installed_at: DateTime<Utc>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(flatten, default)]
    pub extras: HashMap<String, serde_json::Value>,
}

impl HubManifest {
    pub fn load_or_default() -> anyhow::Result<Self> {
        let p = crate::paths::manifest_path()?;
        if !p.exists() {
            return Ok(Self::default());
        }
        Ok(serde_json::from_str(&std::fs::read_to_string(p)?)?)
    }

    #[deprecated(note = "Use SkillLock::save_atomic. HubManifest is migration-read-only in 21.8.")]
    pub fn save(&self) -> anyhow::Result<()> {
        let p = crate::paths::manifest_path()?;
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(tmp, p)?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(deprecated)] // HubManifest::save is deprecated but tests still exercise it for migration parity.
mod tests {
    use super::*;

    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_test_hermes_home<F: FnOnce()>(f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var("HERMES_HOME").ok();
        unsafe {
            std::env::set_var("HERMES_HOME", tmp.path());
        }
        f();
        unsafe {
            match prev {
                Some(v) => std::env::set_var("HERMES_HOME", v),
                None => std::env::remove_var("HERMES_HOME"),
            }
        }
    }

    #[test]
    fn test_manifest_empty_load_default() {
        with_test_hermes_home(|| {
            let m = HubManifest::load_or_default().expect("load");
            assert!(m.installed.is_empty());
        });
    }

    #[test]
    fn test_manifest_roundtrip_preserves_extras() {
        with_test_hermes_home(|| {
            let p = crate::paths::manifest_path().unwrap();
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            let raw = r#"{"installed":{},"future_field":42}"#;
            std::fs::write(&p, raw).unwrap();

            let m = HubManifest::load_or_default().expect("load");
            assert!(m.extras.contains_key("future_field"));
            assert_eq!(m.extras["future_field"], serde_json::json!(42));

            m.save().expect("save");
            let m2 = HubManifest::load_or_default().expect("reload");
            assert_eq!(m2.extras["future_field"], serde_json::json!(42));
        });
    }

    #[test]
    fn test_manifest_save_atomic() {
        with_test_hermes_home(|| {
            let m = HubManifest::default();
            m.save().expect("save1");
            m.save().expect("save2");
            let p = crate::paths::manifest_path().unwrap();
            assert!(p.exists());
            let _m: HubManifest =
                serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        });
    }
}

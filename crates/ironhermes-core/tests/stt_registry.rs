//! Phase 36.17.8 — STT registry unit tests (Plan 02).
//!
//! Covers D-04 (BUILTIN_STT_NAMES), D-06 (registry lookup), D-18 (SttConfig).
//!
//! Run: `cargo test -p ironhermes-core --test stt_registry`

use async_trait::async_trait;
use ironhermes_core::config::{Config, SttConfig};
use ironhermes_core::{BUILTIN_STT_NAMES, SttProvider, SttRegistry};
use std::path::Path;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// FakeSttProvider — inline test double; never touches mic or network.
// ---------------------------------------------------------------------------

struct FakeSttProvider {
    provider_name: &'static str,
}

#[async_trait]
impl SttProvider for FakeSttProvider {
    fn name(&self) -> &str {
        self.provider_name
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn transcribe(&self, _wav_path: &Path) -> anyhow::Result<String> {
        Ok("fake transcript".to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// D-04: BUILTIN_STT_NAMES frozen set contains exactly "groq" and "openai".
#[test]
fn test_builtin_names_set() {
    assert_eq!(BUILTIN_STT_NAMES.len(), 2);
    assert!(BUILTIN_STT_NAMES.contains(&"groq"));
    assert!(BUILTIN_STT_NAMES.contains(&"openai"));
}

/// D-04: Non-builtin name warns and is ignored ("built-ins always win").
#[test]
fn test_stt_registry_builtin_wins() {
    let mut registry = SttRegistry::new();
    let non_builtin = Arc::new(FakeSttProvider {
        provider_name: "whisperx",
    });
    registry.register(non_builtin);
    // The non-builtin registration must be silently dropped.
    assert!(registry.get("whisperx").is_none());
}

/// D-04: Case-insensitive registry lookup (e.g. "GROQ" resolves to the groq provider).
#[test]
fn test_stt_registry_get_groq() {
    let mut registry = SttRegistry::new();
    let groq = Arc::new(FakeSttProvider {
        provider_name: "groq",
    });
    registry.register(groq);
    // Case-insensitive lookup must succeed.
    let found = registry.get("GROQ");
    assert!(found.is_some());
    assert_eq!(found.unwrap().name(), "groq");
}

/// D-18: SttConfig::default() produces the expected default values (D-05/D-06).
#[test]
fn test_stt_config_defaults() {
    let cfg = SttConfig::default();
    assert_eq!(cfg.provider, "auto");
    assert_eq!(cfg.groq.model, "whisper-large-v3-turbo");
    assert_eq!(cfg.openai.model, "whisper-1");
}

/// D-18: YAML round-trip — omitted `stt:` and `voice:` keys in config YAML use defaults.
#[test]
fn test_stt_config_yaml_omitted_uses_defaults() {
    // Empty YAML (no stt: or voice: block) — must parse without error and
    // produce the documented defaults (old-config backward-compat guarantee).
    let yaml = "{}";
    let config: Config = serde_yaml::from_str(yaml).expect("YAML parse failed");
    assert_eq!(config.stt.provider, "auto");
    assert_eq!(config.stt.groq.model, "whisper-large-v3-turbo");
    assert_eq!(config.stt.openai.model, "whisper-1");
    assert_eq!(config.voice.record_key, "ctrl+b");
    assert_eq!(config.voice.silence_threshold, 200);
    assert!((config.voice.silence_duration - 3.0).abs() < f64::EPSILON);
    assert!(!config.voice.auto_tts);
    assert!(config.voice.beep_enabled);
    assert_eq!(config.voice.max_recording_seconds, 120);
}

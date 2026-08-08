//! Phase 36.17.5 — Wave 0 registry unit tests.
//!
//! Covers TTS-01, TTS-02, TTS-07, TTS-08 and the D-12 YAML round-trip.
//! All tests are sync — no tokio runtime needed for registry operations.
//!
//! Run: `cargo test -p ironhermes-core --test tts_registry`

use std::path::PathBuf;
use std::sync::Arc;

use ironhermes_core::Config;
use ironhermes_core::tts::{BUILTIN_TTS_NAMES, TtsProvider, TtsRegistry};

// ---------------------------------------------------------------------------
// Minimal in-test TtsProvider implementation
// ---------------------------------------------------------------------------

struct FakeProvider {
    reported_name: String,
    available: bool,
}

#[async_trait::async_trait]
impl TtsProvider for FakeProvider {
    fn name(&self) -> &str {
        &self.reported_name
    }

    fn is_available(&self) -> bool {
        self.available
    }

    async fn synthesize(&self, _text: &str, _path: &std::path::Path) -> anyhow::Result<PathBuf> {
        anyhow::bail!("fake provider — synthesize not implemented")
    }
}

impl FakeProvider {
    fn new(name: &str, available: bool) -> Arc<Self> {
        Arc::new(Self {
            reported_name: name.to_string(),
            available,
        })
    }
}

// ---------------------------------------------------------------------------
// TTS-08 (D-10): BUILTIN_TTS_NAMES constant shape
// ---------------------------------------------------------------------------

/// TTS-08 (D-10): `BUILTIN_TTS_NAMES` is the compile-time source of truth for the
/// built-in provider set — `register()` accepts only names in it. The set must not
/// gain or lose an entry silently, so this pins it exactly; adding a provider is a
/// deliberate act that must update this list.
///
/// Was written in 36.17.5 when the set was `["edge", "elevenlabs"]`, and pinned the
/// literal AND the length AND each index. Phase 40.5 (`6ad469dc7`) legitimately added
/// `OpenAiTtsProvider` to the constant without updating the test, so it has been red
/// ever since — masked because `cargo test -p ironhermes-core` aborts at the first
/// failing target and two earlier lib tests were failing. Dropped the redundant
/// length/index assertions: they restate the set equality and were pure churn.
#[test]
fn test_builtin_names_set() {
    assert_eq!(
        BUILTIN_TTS_NAMES,
        &["edge", "elevenlabs", "openai"],
        "TTS-08 (D-10): BUILTIN_TTS_NAMES is the source of truth for the built-in set. \
         If you added or removed a provider, update this list deliberately; if you did \
         not, something registered a provider it should not have."
    );

    let mut sorted = BUILTIN_TTS_NAMES.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        BUILTIN_TTS_NAMES.len(),
        "TTS-08 (D-10): BUILTIN_TTS_NAMES must not contain duplicates — a dupe would let \
         register() accept the same name twice"
    );
}

// ---------------------------------------------------------------------------
// TTS-01 (D-10): built-ins-always-win — non-built-in names are warn-and-ignored
// ---------------------------------------------------------------------------

#[test]
fn test_tts_registry_builtin_wins() {
    let mut registry = TtsRegistry::new();

    // Non-built-in name should be rejected (warn-and-ignore)
    let fake_non_builtin = FakeProvider::new("fake", true);
    registry.register(fake_non_builtin);
    assert!(
        registry.get("fake").is_none(),
        "TTS-01 (D-10): non-built-in name 'fake' must be rejected by register()"
    );

    // Built-in name "edge" must be accepted unconditionally
    let fake_edge = FakeProvider::new("edge", true);
    registry.register(fake_edge);
    assert!(
        registry.get("edge").is_some(),
        "TTS-01 (D-10): built-in name 'edge' must be inserted by register()"
    );

    // Built-in name "elevenlabs" must also be accepted
    let fake_elevenlabs = FakeProvider::new("elevenlabs", false);
    registry.register(fake_elevenlabs);
    assert!(
        registry.get("elevenlabs").is_some(),
        "TTS-01 (D-10): built-in name 'elevenlabs' must be inserted by register()"
    );
}

// ---------------------------------------------------------------------------
// TTS-02 (D-10): case-insensitive lookup
// ---------------------------------------------------------------------------

#[test]
fn test_tts_registry_get_edge() {
    let mut registry = TtsRegistry::new();

    // Register with uppercase name — should be lowercased on insert
    let provider = FakeProvider::new("Edge", true);
    registry.register(provider);

    assert!(
        registry.get("edge").is_some(),
        "TTS-02 (D-10): get(\"edge\") must return Some after registering \"Edge\""
    );
    assert!(
        registry.get("EDGE").is_some(),
        "TTS-02 (D-10): get(\"EDGE\") must return Some (case-insensitive lookup)"
    );
    assert!(
        registry.get("Edge").is_some(),
        "TTS-02 (D-10): get(\"Edge\") must return Some (case-insensitive lookup)"
    );
}

// ---------------------------------------------------------------------------
// TTS-07 (D-12): TtsConfig::default() returns correct values
// ---------------------------------------------------------------------------

#[test]
fn test_tts_config_defaults() {
    let config = Config::default();

    assert_eq!(
        config.tts.provider, "edge",
        "TTS-07 (D-12): default TTS provider must be \"edge\""
    );
    assert_eq!(
        config.tts.edge.voice, "en-US-AriaNeural",
        "TTS-07 (D-12): default Edge voice must be \"en-US-AriaNeural\""
    );
    assert_eq!(
        config.tts.elevenlabs.voice_id, "pNInz6obpgDQGcFmaJgB",
        "TTS-07 (D-12): default ElevenLabs voice_id must be \"pNInz6obpgDQGcFmaJgB\" (Adam)"
    );
    assert_eq!(
        config.tts.elevenlabs.model_id, "eleven_multilingual_v2",
        "TTS-07 (D-12): default ElevenLabs model_id must be \"eleven_multilingual_v2\""
    );
    assert!(
        config.tts.ffmpeg_path.is_none(),
        "TTS-07 (D-12): default ffmpeg_path must be None"
    );
}

// ---------------------------------------------------------------------------
// TTS-07 extended (D-12): YAML round-trip — omitted tts: block uses defaults
// ---------------------------------------------------------------------------

#[test]
fn test_tts_config_yaml_omitted_block_uses_defaults() {
    // A config YAML that does NOT contain a `tts:` key at all.
    // `#[serde(default)]` on the field must fill in all TtsConfig defaults.
    // Use a truly empty doc so we don't need to satisfy other field shapes.
    let yaml = "{}\n";
    let config: Config = serde_yaml::from_str(yaml)
        .expect("TTS-07 (D-12): Config must parse cleanly even when tts: block is absent");

    assert_eq!(
        config.tts.provider, "edge",
        "TTS-07 (D-12): YAML round-trip — omitted tts.provider must default to \"edge\""
    );
    assert_eq!(
        config.tts.edge.voice, "en-US-AriaNeural",
        "TTS-07 (D-12): YAML round-trip — omitted tts.edge.voice must default to \"en-US-AriaNeural\""
    );
    assert_eq!(
        config.tts.elevenlabs.voice_id, "pNInz6obpgDQGcFmaJgB",
        "TTS-07 (D-12): YAML round-trip — omitted tts.elevenlabs.voice_id must default to \"pNInz6obpgDQGcFmaJgB\""
    );
    assert_eq!(
        config.tts.elevenlabs.model_id, "eleven_multilingual_v2",
        "TTS-07 (D-12): YAML round-trip — omitted tts.elevenlabs.model_id must default to \"eleven_multilingual_v2\""
    );
}

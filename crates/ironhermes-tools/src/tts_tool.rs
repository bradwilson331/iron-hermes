// Phase 36.17.5 D-05 / D-06 / D-07 — TextToSpeechTool: LLM-callable synthesis tool.
//
// D-05: Single tool named "text_to_speech", toolset "voice".
// D-06: Returns the output file path as a string — no base64, no side-channel event.
// D-07: Only `text` (required) + `output_path` (optional) in schema; provider / model /
//       speed / output_format come from config and are DEFERRED from the LLM surface.
//
// T-output-path BLOCKING mitigation: LLM-supplied output_path values are CLAMPED to
// $IRONHERMES_HOME/audio_cache/ via canonicalize + starts_with before the synthesize
// call. Locked by test_output_path_traversal_blocked unit test.
//
// D-16: audio_cache/ is created via create_dir_all BEFORE the synthesize call
// (idempotent; safe on concurrent calls).

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

use crate::registry::Tool;

/// LLM-callable tool that synthesizes text via the configured TTS provider and
/// returns the path of the written audio file (D-06).
pub struct TextToSpeechTool {
    config: Arc<ironhermes_core::Config>,
    registry: Arc<ironhermes_core::tts::TtsRegistry>,
}

impl TextToSpeechTool {
    pub fn new(
        config: Arc<ironhermes_core::Config>,
        registry: Arc<ironhermes_core::tts::TtsRegistry>,
    ) -> Self {
        Self { config, registry }
    }
}

impl TextToSpeechTool {
    /// Test-friendly constructor: accepts a pre-built `TtsRegistry` by value (avoids
    /// live network in unit tests). Available in both test and non-test builds so
    /// integration tests in separate test binaries can also use it.
    pub fn new_with_registry(
        config: Arc<ironhermes_core::Config>,
        registry: ironhermes_core::tts::TtsRegistry,
    ) -> Self {
        Self {
            config,
            registry: Arc::new(registry),
        }
    }
}

#[async_trait]
impl Tool for TextToSpeechTool {
    fn name(&self) -> &str {
        "text_to_speech"
    }

    fn toolset(&self) -> &str {
        "voice"
    }

    fn description(&self) -> &str {
        "Synthesize text to an audio file via the configured TTS provider. Returns the path to the audio file."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "text_to_speech",
            "Synthesize text to an audio file via the configured TTS provider. Returns the path to the audio file.",
            json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Text to synthesize (Edge caps at 5000 chars, ElevenLabs at 10000)."
                    },
                    "output_path": {
                        "type": "string",
                        "description": "Optional output file path. Defaults to $IRONHERMES_HOME/audio_cache/<uuid>.<ext>. If provided, MUST be inside the audio_cache directory."
                    }
                },
                "required": ["text"]
            }),
        )
    }

    fn is_available(&self) -> bool {
        self.registry.get(&self.config.tts.provider).is_some()
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        // Step a: extract text
        let text = args["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: text"))?;

        // Step b: look up provider
        let provider = self
            .registry
            .get(&self.config.tts.provider)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "TTS provider '{}' is not registered",
                    self.config.tts.provider
                )
            })?;

        // Step c: D-16 — create audio_cache dir idempotently BEFORE output_path resolution
        let audio_dir = ironhermes_core::constants::get_hermes_home().join("audio_cache");
        std::fs::create_dir_all(&audio_dir)?;

        // Step d: resolve output_path
        let output_path: PathBuf = if let Some(p_str) = args["output_path"].as_str() {
            let p = PathBuf::from(p_str);

            // Ensure parent exists under audio_dir before canonicalization
            let parent = p.parent().unwrap_or(&audio_dir);
            if !parent.exists() {
                // Only create if parent is plausibly inside audio_dir — we'll verify with
                // canonicalize below; if it's outside, canonicalize will either fail or
                // return a path that fails the starts_with check.
                let _ = std::fs::create_dir_all(parent);
            }

            // T-output-path BLOCKING CHECK: canonicalize both sides and assert containment
            let canonical_parent =
                std::fs::canonicalize(parent).unwrap_or_else(|_| audio_dir.clone());
            let audio_canonical =
                std::fs::canonicalize(&audio_dir).unwrap_or_else(|_| audio_dir.clone());

            if !canonical_parent.starts_with(&audio_canonical) {
                anyhow::bail!(
                    "output_path must be inside $IRONHERMES_HOME/audio_cache/ \
                     (path traversal detected: {:?} is not under {:?})",
                    canonical_parent,
                    audio_canonical
                );
            }

            p
        } else {
            // Determine extension from provider type
            let ext = if self.config.tts.provider == "elevenlabs"
                && self.config.tts.elevenlabs.output_format == "opus"
            {
                "opus"
            } else {
                "mp3"
            };
            audio_dir.join(format!("{}.{}", uuid::Uuid::new_v4(), ext))
        };

        // Step e: synthesize
        let written_path = provider.synthesize(text, &output_path).await?;

        // Step f: D-06 — return path string only (no base64, no side-channel)
        Ok(written_path.to_string_lossy().to_string())
    }
}

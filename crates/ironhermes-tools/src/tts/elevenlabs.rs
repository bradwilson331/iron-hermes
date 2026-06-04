// Phase 36.17.5 — ElevenLabsProvider placeholder (Task 1)
// Full TtsProvider impl is completed in Task 3.

use ironhermes_core::config::ElevenLabsConfig;

/// ElevenLabs TTS provider via REST API (D-03: premium, ELEVENLABS_API_KEY).
pub struct ElevenLabsProvider {
    pub config: ElevenLabsConfig,
}

impl ElevenLabsProvider {
    pub fn new(config: ElevenLabsConfig) -> Self {
        Self { config }
    }
}

// Stub impl so build_tts_registry() in mod.rs compiles at Task 1 stage.
// Task 3 replaces this with the real implementation.
#[async_trait::async_trait]
impl ironhermes_core::tts::TtsProvider for ElevenLabsProvider {
    fn name(&self) -> &str {
        "elevenlabs"
    }

    fn is_available(&self) -> bool {
        std::env::var("ELEVENLABS_API_KEY").is_ok()
    }

    async fn synthesize(
        &self,
        _text: &str,
        _output_path: &std::path::PathBuf,
    ) -> anyhow::Result<std::path::PathBuf> {
        unimplemented!("ElevenLabsProvider::synthesize — implemented in Task 3")
    }
}

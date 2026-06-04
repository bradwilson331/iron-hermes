// Phase 36.17.5 — EdgeProvider placeholder (Task 1)
// Full TtsProvider impl is completed in Task 2.

use ironhermes_core::config::EdgeTtsConfig;

/// Edge TTS provider wrapping `msedge-tts` (D-03: free default, no API key).
pub struct EdgeProvider {
    pub config: EdgeTtsConfig,
}

impl EdgeProvider {
    pub fn new(config: EdgeTtsConfig) -> Self {
        Self { config }
    }
}

// Stub impl so build_tts_registry() in mod.rs compiles at Task 1 stage.
// Task 2 replaces this with the real implementation.
#[async_trait::async_trait]
impl ironhermes_core::tts::TtsProvider for EdgeProvider {
    fn name(&self) -> &str {
        "edge"
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn synthesize(
        &self,
        _text: &str,
        _output_path: &std::path::PathBuf,
    ) -> anyhow::Result<std::path::PathBuf> {
        unimplemented!("EdgeProvider::synthesize — implemented in Task 2")
    }
}

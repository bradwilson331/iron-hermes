// Phase 36.17.8 — OpenAI STT provider (D-03, D-05, D-06, D-07)
//
// D-07: is_available() returns true iff VOICE_TOOLS_OPENAI_KEY is set in the environment.
// D-05: STT_OPENAI_MODEL env var overrides the configured / default model.
// T-36.17.8-key-leak: API key read from env only, passed to reqwest bearer_auth,
//                     never logged, traced, or printed.

use async_trait::async_trait;
use ironhermes_core::SttProvider;
use ironhermes_core::config::OpenAiSttConfig;
use std::path::Path;

use crate::stt::whisper_api_transcribe;

/// OpenAI Whisper STT provider.
///
/// Transcribes WAV audio via `POST https://api.openai.com/v1/audio/transcriptions`.
/// Requires `VOICE_TOOLS_OPENAI_KEY` in the environment. When unset, `is_available()`
/// returns false.
pub struct OpenAiSttProvider {
    pub config: OpenAiSttConfig,
}

impl OpenAiSttProvider {
    pub fn new(config: OpenAiSttConfig) -> Self {
        Self { config }
    }

    /// Resolve the endpoint — honours `OPENAI_STT_BASE_URL` env override for testing.
    fn endpoint() -> String {
        std::env::var("OPENAI_STT_BASE_URL")
            .map(|base| format!("{}/v1/audio/transcriptions", base.trim_end_matches('/')))
            .unwrap_or_else(|_| "https://api.openai.com/v1/audio/transcriptions".to_string())
    }
}

#[async_trait]
impl SttProvider for OpenAiSttProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn display_name(&self) -> &str {
        "OpenAI Whisper"
    }

    /// Returns `true` iff `VOICE_TOOLS_OPENAI_KEY` is set in the environment.
    ///
    /// Verbatim mirror of `web_search.rs` is_available() pattern (D-07).
    fn is_available(&self) -> bool {
        std::env::var("VOICE_TOOLS_OPENAI_KEY").is_ok()
    }

    /// Transcribe the WAV at `wav_path` via the OpenAI Whisper endpoint.
    ///
    /// Model priority: `STT_OPENAI_MODEL` env var → `config.model` → `"whisper-1"`.
    async fn transcribe(&self, wav_path: &Path) -> anyhow::Result<String> {
        // T-36.17.8-key-leak: read from env, never log or trace the value.
        let api_key = std::env::var("VOICE_TOOLS_OPENAI_KEY")
            .map_err(|_| anyhow::anyhow!("VOICE_TOOLS_OPENAI_KEY environment variable not set"))?;

        // D-05: STT_OPENAI_MODEL overrides configured / default.
        let model = std::env::var("STT_OPENAI_MODEL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if self.config.model.is_empty() {
                    "whisper-1".to_string()
                } else {
                    self.config.model.clone()
                }
            });

        whisper_api_transcribe(&Self::endpoint(), &api_key, &model, wav_path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::config::OpenAiSttConfig;

    fn default_provider() -> OpenAiSttProvider {
        OpenAiSttProvider::new(OpenAiSttConfig::default())
    }

    #[test]
    fn test_name_is_openai() {
        assert_eq!(default_provider().name(), "openai");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(default_provider().display_name(), "OpenAI Whisper");
    }

    #[test]
    fn test_is_available_when_key_set() {
        // Safety: single-threaded test; restored on all paths.
        let orig = std::env::var("VOICE_TOOLS_OPENAI_KEY").ok();
        unsafe { std::env::set_var("VOICE_TOOLS_OPENAI_KEY", "test-key") };
        let result = default_provider().is_available();
        match orig {
            Some(v) => unsafe { std::env::set_var("VOICE_TOOLS_OPENAI_KEY", v) },
            None => unsafe { std::env::remove_var("VOICE_TOOLS_OPENAI_KEY") },
        }
        assert!(
            result,
            "is_available() should be true when VOICE_TOOLS_OPENAI_KEY is set"
        );
    }

    #[test]
    fn test_is_available_when_key_unset() {
        let orig = std::env::var("VOICE_TOOLS_OPENAI_KEY").ok();
        unsafe { std::env::remove_var("VOICE_TOOLS_OPENAI_KEY") };
        let result = default_provider().is_available();
        if let Some(v) = orig {
            unsafe { std::env::set_var("VOICE_TOOLS_OPENAI_KEY", v) };
        }
        assert!(
            !result,
            "is_available() should be false when VOICE_TOOLS_OPENAI_KEY is unset"
        );
    }
}

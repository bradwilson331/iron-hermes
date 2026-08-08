// Phase 36.17.8 — Groq STT provider (D-03, D-05, D-06, D-07)
//
// D-07: is_available() returns true iff GROQ_API_KEY is set in the environment.
// D-05: STT_GROQ_MODEL env var overrides the configured / default model.
// T-36.17.8-key-leak: API key read from env only, passed to reqwest bearer_auth,
//                     never logged, traced, or printed.

use async_trait::async_trait;
use ironhermes_core::SttProvider;
use ironhermes_core::config::GroqSttConfig;
use std::path::Path;

use crate::stt::whisper_api_transcribe;

/// Groq Whisper STT provider.
///
/// Transcribes WAV audio via `POST https://api.groq.com/openai/v1/audio/transcriptions`.
/// Requires `GROQ_API_KEY` in the environment. When unset, `is_available()` returns false.
pub struct GroqSttProvider {
    pub config: GroqSttConfig,
}

impl GroqSttProvider {
    pub fn new(config: GroqSttConfig) -> Self {
        Self { config }
    }

    /// Resolve the endpoint — honours `GROQ_BASE_URL` env override for testing
    /// (RESEARCH Landmine #7: wiremock points there).
    fn endpoint() -> String {
        std::env::var("GROQ_BASE_URL")
            .map(|base| {
                format!(
                    "{}/openai/v1/audio/transcriptions",
                    base.trim_end_matches('/')
                )
            })
            .unwrap_or_else(|_| "https://api.groq.com/openai/v1/audio/transcriptions".to_string())
    }
}

#[async_trait]
impl SttProvider for GroqSttProvider {
    fn name(&self) -> &str {
        "groq"
    }

    fn display_name(&self) -> &str {
        "Groq Whisper"
    }

    /// Returns `true` iff `GROQ_API_KEY` is set in the environment.
    ///
    /// Verbatim mirror of `web_search.rs` is_available() pattern (D-07).
    fn is_available(&self) -> bool {
        std::env::var("GROQ_API_KEY").is_ok()
    }

    /// Transcribe the WAV at `wav_path` via the Groq Whisper endpoint.
    ///
    /// Model priority: `STT_GROQ_MODEL` env var → `config.model` → `"whisper-large-v3-turbo"`.
    async fn transcribe(&self, wav_path: &Path) -> anyhow::Result<String> {
        // T-36.17.8-key-leak: read from env, never log or trace the value.
        let api_key = std::env::var("GROQ_API_KEY")
            .map_err(|_| anyhow::anyhow!("GROQ_API_KEY environment variable not set"))?;

        // D-05: STT_GROQ_MODEL overrides configured / default.
        let model = std::env::var("STT_GROQ_MODEL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if self.config.model.is_empty() {
                    "whisper-large-v3-turbo".to_string()
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
    use ironhermes_core::config::GroqSttConfig;

    fn default_provider() -> GroqSttProvider {
        GroqSttProvider::new(GroqSttConfig::default())
    }

    #[test]
    fn test_name_is_groq() {
        assert_eq!(default_provider().name(), "groq");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(default_provider().display_name(), "Groq Whisper");
    }

    #[test]
    fn test_is_available_when_key_set() {
        // Safety: single-threaded test; restored on all paths.
        let orig = std::env::var("GROQ_API_KEY").ok();
        unsafe { std::env::set_var("GROQ_API_KEY", "test-key") };
        let result = default_provider().is_available();
        match orig {
            Some(v) => unsafe { std::env::set_var("GROQ_API_KEY", v) },
            None => unsafe { std::env::remove_var("GROQ_API_KEY") },
        }
        assert!(
            result,
            "is_available() should be true when GROQ_API_KEY is set"
        );
    }

    #[test]
    fn test_is_available_when_key_unset() {
        let orig = std::env::var("GROQ_API_KEY").ok();
        unsafe { std::env::remove_var("GROQ_API_KEY") };
        let result = default_provider().is_available();
        if let Some(v) = orig {
            unsafe { std::env::set_var("GROQ_API_KEY", v) };
        }
        assert!(
            !result,
            "is_available() should be false when GROQ_API_KEY is unset"
        );
    }
}

// Phase 40.5 Plan 03 Task 1 — OpenAiTtsProvider (D-10)
//
// D-10: An OpenAiTtsProvider is built mirroring ElevenLabsProvider exactly:
//       - struct holding OpenAiTtsConfig (model, voice, format)
//       - is_available() returns true iff OPENAI_API_KEY is set in env
//       - synthesize(text, output_path): server-side POST to OpenAI speech endpoint,
//         reads OPENAI_API_KEY from env, writes audio bytes to output_path.
//       - truncate_text() caps at 4096 chars (OpenAI /v1/audio/speech limit).
//
// T-40.5-03-02: API key read only from env, never placed in any returned value,
//               error message, or log entry. The binding `api_key` is used solely
//               as a Bearer header value inside reqwest and is not captured elsewhere.
// T-40.5-03-06: truncate_text() bounds cost-blowup by capping input at 4096 chars
//               before the HTTP call.

use async_trait::async_trait;
use ironhermes_core::config::OpenAiTtsConfig;
use ironhermes_core::tts::TtsProvider;
use std::path::{Path, PathBuf};

/// OpenAI TTS provider using the `/v1/audio/speech` REST endpoint.
///
/// Requires `OPENAI_API_KEY` environment variable. When unset,
/// `is_available()` returns false and the provider cannot synthesize.
pub struct OpenAiTtsProvider {
    pub config: OpenAiTtsConfig,
}

impl OpenAiTtsProvider {
    pub fn new(config: OpenAiTtsConfig) -> Self {
        Self { config }
    }

    /// Truncate text to the OpenAI `/v1/audio/speech` cap (4096 chars).
    ///
    /// Truncating client-side avoids API errors and bounds cost-blowup
    /// attacks (T-40.5-03-06 mitigation). Uses char-boundary-aware slice.
    pub(crate) fn truncate_text(text: &str) -> &str {
        const OPENAI_SPEECH_LIMIT: usize = 4096;
        if text.len() > OPENAI_SPEECH_LIMIT {
            let mut end = OPENAI_SPEECH_LIMIT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            &text[..end]
        } else {
            text
        }
    }
}

#[async_trait]
impl TtsProvider for OpenAiTtsProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn display_name(&self) -> &str {
        "OpenAI"
    }

    /// Returns `true` iff `OPENAI_API_KEY` is set in the environment.
    ///
    /// Verbatim mirror of ElevenLabsProvider::is_available() (D-10).
    fn is_available(&self) -> bool {
        std::env::var("OPENAI_API_KEY").is_ok()
    }

    /// Returns false for MP3 output (v1 — voice-bubble Opus deferred).
    fn voice_compatible(&self) -> bool {
        false
    }

    /// Synthesize `text` to `output_path` using the OpenAI TTS REST API.
    ///
    /// POST https://api.openai.com/v1/audio/speech
    /// Header: Authorization: Bearer <OPENAI_API_KEY>
    /// Body: { "model": "...", "voice": "...", "input": "...", "response_format": "mp3" }
    /// Response 200: raw audio bytes
    /// Response 4xx/5xx: JSON error body surfaced in anyhow error (key NOT included).
    async fn synthesize(&self, text: &str, output_path: &Path) -> anyhow::Result<PathBuf> {
        let text = Self::truncate_text(text);

        // T-40.5-03-02: key read from env server-side; the value is used only as a
        // reqwest header and is never placed in any returned value or error string.
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| anyhow::anyhow!("OPENAI_API_KEY environment variable not set"))?;

        // Per-call client — mirrors ElevenLabsProvider pattern.
        let client = reqwest::Client::new();

        let response_format = if self.config.format.is_empty() {
            "mp3".to_string()
        } else {
            self.config.format.clone()
        };

        let response = client
            .post("https://api.openai.com/v1/audio/speech")
            // T-40.5-03-02: key used here only; format! string never includes its value.
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
            .json(&serde_json::json!({
                "model": self.config.model,
                "voice": self.config.voice,
                "input": text,
                "response_format": response_format
            }))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("OpenAI TTS HTTP request failed: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            // Surface response body for diagnostics (quota/voice errors).
            // Key value is NOT included — only the status code and server-sent body.
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("OpenAI TTS API returned {status}: {body}"));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read OpenAI TTS response body: {e}"))?;

        tokio::fs::write(output_path, &bytes)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write audio file: {e}"))?;

        Ok(output_path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::build_tts_registry;
    use ironhermes_core::config::TtsConfig;
    use ironhermes_core::tts::BUILTIN_TTS_NAMES;
    use std::sync::Mutex;

    /// Mutex to serialise tests that mutate `OPENAI_API_KEY`.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn default_provider() -> OpenAiTtsProvider {
        OpenAiTtsProvider::new(OpenAiTtsConfig::default())
    }

    #[test]
    fn test_truncate_short() {
        let s = "Hello, world!";
        assert_eq!(OpenAiTtsProvider::truncate_text(s), s);
    }

    #[test]
    fn test_truncate_long() {
        // Build a 6000-char ASCII string — above the 4096-char limit.
        let s: String = "a".repeat(6000);
        let truncated = OpenAiTtsProvider::truncate_text(&s);
        assert_eq!(truncated.len(), 4096);
    }

    /// T-D10: is_available() returns false when OPENAI_API_KEY is unset.
    #[test]
    fn test_openai_unavailable_no_key() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prior = std::env::var("OPENAI_API_KEY").ok();
        // SAFETY: single-threaded access via ENV_LOCK.
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
        let result = default_provider().is_available();
        if let Some(val) = prior {
            unsafe { std::env::set_var("OPENAI_API_KEY", val) };
        }
        assert!(
            !result,
            "is_available() must be false when OPENAI_API_KEY is unset"
        );
    }

    /// D-10: provider name must be the canonical lowercase string "openai".
    #[test]
    fn test_openai_name_is_openai() {
        assert_eq!(default_provider().name(), "openai");
    }

    /// D-10: display name must be "OpenAI".
    #[test]
    fn test_openai_display_name() {
        assert_eq!(default_provider().display_name(), "OpenAI");
    }

    /// D-10: voice_compatible() must be false for MP3 output.
    #[test]
    fn test_openai_voice_compatible_is_false() {
        assert!(!default_provider().voice_compatible());
    }

    /// D-10: BUILTIN_TTS_NAMES must contain "openai" so TtsRegistry::register accepts it.
    #[test]
    fn test_openai_registered_in_builtin() {
        assert!(
            BUILTIN_TTS_NAMES.contains(&"openai"),
            "'openai' must be in BUILTIN_TTS_NAMES"
        );
    }

    /// D-10: build_tts_registry must register the provider so .get("openai") is Some.
    #[test]
    fn test_registry_has_openai() {
        let registry = build_tts_registry(&TtsConfig::default());
        assert!(
            registry.get("openai").is_some(),
            "build_tts_registry must register the 'openai' provider"
        );
    }
}

// Phase 36.17.5 — ElevenLabsProvider: premium TTS via ElevenLabs REST API (D-03)
//
// D-03: ElevenLabsProvider::is_available() returns true iff ELEVENLABS_API_KEY is set.
//       Mirrors web_search.rs FIRECRAWL_API_KEY pattern exactly.
// D-09: Implements the minimal TtsProvider surface (name, display_name,
//       is_available, voice_compatible, synthesize). DEFERRED: stream(),
//       list_voices(), list_models(), etc.
// D-11: Error type is anyhow::Result — no typed TtsError enum.
// T-text-length: truncate_text() caps input at 10000 chars (eleven_multilingual_v2 limit)
//               before the HTTP call to bound LLM cost-blowup attacks.
// T-api-key-leak: xi-api-key header is passed directly to reqwest HeaderMap and is
//                never written to logs or trajectory output.

use async_trait::async_trait;
use ironhermes_core::config::ElevenLabsConfig;
use ironhermes_core::tts::TtsProvider;
use std::path::{Path, PathBuf};

/// ElevenLabs TTS provider using the REST API.
///
/// Requires `ELEVENLABS_API_KEY` environment variable. When unset,
/// `is_available()` returns false and the provider cannot synthesize.
pub struct ElevenLabsProvider {
    pub config: ElevenLabsConfig,
}

impl ElevenLabsProvider {
    pub fn new(config: ElevenLabsConfig) -> Self {
        Self { config }
    }

    /// Truncate text to the ElevenLabs cap for `eleven_multilingual_v2` (10000 chars).
    ///
    /// Truncating client-side avoids API errors and bounds LLM cost-blowup
    /// attacks (T-text-length mitigation). Uses char-boundary-aware truncation.
    pub(crate) fn truncate_text(text: &str) -> &str {
        if text.len() > 10000 {
            // Find the last valid char boundary at or before byte 10000.
            let mut end = 10000;
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
impl TtsProvider for ElevenLabsProvider {
    fn name(&self) -> &str {
        "elevenlabs"
    }

    fn display_name(&self) -> &str {
        "ElevenLabs"
    }

    /// Returns true iff `ELEVENLABS_API_KEY` is set in the environment.
    ///
    /// Verbatim mirror of `web_search.rs` is_available() pattern (D-03).
    fn is_available(&self) -> bool {
        std::env::var("ELEVENLABS_API_KEY").is_ok()
    }

    /// Returns false for v1 (MP3 output).
    ///
    /// ElevenLabs Opus voice-bubble support deferred: requires OGG container
    /// handling and `output_format: "opus_48000_128"` wiring (RESEARCH Open Q #2).
    fn voice_compatible(&self) -> bool {
        false
    }

    /// Synthesize `text` to `output_path` using the ElevenLabs v1 REST API.
    ///
    /// POST https://api.elevenlabs.io/v1/text-to-speech/{voice_id}
    /// Header: xi-api-key: <ELEVENLABS_API_KEY>
    /// Body: { "text": "...", "model_id": "...", "output_format": "mp3_44100_128" }
    /// Response 200: raw MP3 audio bytes (application/octet-stream)
    /// Response 4xx/5xx: { "detail": { "type", "code", "message", "request_id" } }
    async fn synthesize(&self, text: &str, output_path: &Path) -> anyhow::Result<PathBuf> {
        let text = Self::truncate_text(text);

        let api_key = std::env::var("ELEVENLABS_API_KEY")
            .map_err(|_| anyhow::anyhow!("ELEVENLABS_API_KEY environment variable not set"))?;

        // Per-call client — mirrors web_search.rs line 92 pattern.
        let client = reqwest::Client::new();

        let url = format!(
            "https://api.elevenlabs.io/v1/text-to-speech/{}",
            self.config.voice_id
        );

        // output_format hardcoded to MP3 for v1 per RESEARCH Open Q #2 deferral.
        // config.output_format is reserved for a future Opus-opt-in phase.
        let response = client
            .post(&url)
            .header("xi-api-key", &api_key)
            .json(&serde_json::json!({
                "text": text,
                "model_id": self.config.model_id,
                "output_format": "mp3_44100_128"
            }))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ElevenLabs HTTP request failed: {}", e))?;

        let status = response.status();
        if !status.is_success() {
            // Surface full JSON body — includes detail.code which lets operator
            // distinguish quota_exceeded vs rate_limit_exceeded vs invalid_voice_id
            // (RESEARCH Pitfall 2). D-11: anyhow, not typed error.
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "ElevenLabs API returned {}: {}",
                status,
                body
            ));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read ElevenLabs response body: {}", e))?;

        // Never persist an empty audio body: a 200 with no bytes would write a
        // silent 0-byte mp3 that is reported as success (debug:
        // web-tts-zero-byte-audio). Surface it as an error instead.
        if bytes.is_empty() {
            return Err(anyhow::anyhow!(
                "ElevenLabs returned an empty audio body (HTTP {status})"
            ));
        }

        tokio::fs::write(output_path, &bytes)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write audio file: {}", e))?;

        Ok(output_path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::config::ElevenLabsConfig;

    fn default_provider() -> ElevenLabsProvider {
        ElevenLabsProvider::new(ElevenLabsConfig::default())
    }

    #[test]
    fn test_truncate_short() {
        let s = "Hello, world!";
        assert_eq!(ElevenLabsProvider::truncate_text(s), s);
    }

    #[test]
    fn test_truncate_long() {
        // Build a 12000-char ASCII string.
        let s: String = "a".repeat(12000);
        let truncated = ElevenLabsProvider::truncate_text(&s);
        assert_eq!(truncated.len(), 10000);
    }

    #[test]
    fn test_name_is_elevenlabs() {
        assert_eq!(default_provider().name(), "elevenlabs");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(default_provider().display_name(), "ElevenLabs");
    }

    #[test]
    fn test_voice_compatible_is_false() {
        assert!(!default_provider().voice_compatible());
    }
}

// Phase 36.17.5 — EdgeProvider: free default TTS via msedge-tts (D-03)
//
// D-03: EdgeProvider::is_available() is always true — no API key required.
// D-09: Implements the minimal TtsProvider surface (name, display_name,
//       is_available, voice_compatible, synthesize). DEFERRED: stream(),
//       list_voices(), list_models(), etc.
// D-11: Error type is anyhow::Result — no typed TtsError enum.
// T-text-length: truncate_text() caps input at 5000 chars before the network
//               call to avoid noisy Edge server 4xx and bound LLM cost.

use async_trait::async_trait;
use ironhermes_core::config::EdgeTtsConfig;
use ironhermes_core::tts::TtsProvider;
use std::path::{Path, PathBuf};

/// Edge TTS provider wrapping `msedge-tts` crate.
///
/// Uses Microsoft Edge's Read Aloud WebSocket API; no API key required.
/// Output format is MP3 (`audio-24khz-48kbitrate-mono-mp3`).
pub struct EdgeProvider {
    pub config: EdgeTtsConfig,
}

impl EdgeProvider {
    pub fn new(config: EdgeTtsConfig) -> Self {
        Self { config }
    }

    /// Truncate text to the Edge TTS server-side cap (5000 chars).
    ///
    /// Truncating client-side avoids noisy 4xx errors from the Edge endpoint
    /// and bounds LLM cost-blowup attacks (T-text-length mitigation).
    /// The slice is safe because UTF-8 chars may span multiple bytes; we use
    /// a char-boundary-aware truncation.
    pub(crate) fn truncate_text(text: &str) -> &str {
        if text.len() > 5000 {
            // Find the last valid char boundary at or before byte 5000.
            let mut end = 5000;
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
impl TtsProvider for EdgeProvider {
    fn name(&self) -> &str {
        "edge"
    }

    fn display_name(&self) -> &str {
        "Microsoft Edge TTS"
    }

    /// Always true — Edge TTS requires no API key (D-03).
    fn is_available(&self) -> bool {
        true
    }

    /// Returns false: Edge TTS emits MP3, not Opus/OGG.
    ///
    /// Telegram voice bubbles need OGG/Opus; without ffmpeg the SendAudioTool
    /// falls back to send_audio (rectangular music player). See D-04.
    fn voice_compatible(&self) -> bool {
        false
    }

    /// Synthesize `text` to `output_path` using the Edge TTS WebSocket API.
    ///
    /// API deviation (msedge-tts 0.4 vs 0.3 plan):
    /// - connect fn: `msedge_tts::tts::client::tokio_runtime::connect_async()`
    /// - SpeechConfig: constructed directly (not via AudioFormat enum constant;
    ///   the crate uses plain String for audio_format in v0.4)
    /// - MP3 format string: `"audio-24khz-48kbitrate-mono-mp3"` (confirmed from
    ///   crate source — From<&Voice> fallback default)
    /// - audio_bytes: field on SynthesizedAudio (confirmed from crate source)
    async fn synthesize(&self, text: &str, output_path: &Path) -> anyhow::Result<PathBuf> {
        let text = Self::truncate_text(text);

        let voice_name = if self.config.voice.is_empty() {
            "en-US-AriaNeural".to_string()
        } else {
            self.config.voice.clone()
        };

        // Build SpeechConfig directly — msedge-tts 0.4 uses plain String fields
        // for audio_format, not an AudioFormat enum.
        let speech_config = msedge_tts::tts::SpeechConfig {
            voice_name,
            audio_format: "audio-24khz-48kbitrate-mono-mp3".to_string(),
            pitch: 0,
            rate: 0,
            volume: 0,
        };

        // Open a new WebSocket connection per call (stateless, matches project
        // convention of per-call clients — mirrors web_search.rs reqwest pattern).
        let mut client =
            msedge_tts::tts::client::tokio_runtime::connect_async()
                .await
                .map_err(|e| anyhow::anyhow!("Edge TTS connect failed: {}", e))?;

        let audio = client
            .synthesize(text, &speech_config)
            .await
            .map_err(|e| anyhow::anyhow!("Edge TTS synthesis failed: {}", e))?;

        tokio::fs::write(output_path, &audio.audio_bytes)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write Edge audio file: {}", e))?;

        Ok(output_path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::config::EdgeTtsConfig;

    fn default_provider() -> EdgeProvider {
        EdgeProvider::new(EdgeTtsConfig::default())
    }

    #[test]
    fn test_truncate_short() {
        let s = "Hello, world!";
        assert_eq!(EdgeProvider::truncate_text(s), s);
    }

    #[test]
    fn test_truncate_long() {
        // Build a 6000-char ASCII string.
        let s: String = "a".repeat(6000);
        let truncated = EdgeProvider::truncate_text(&s);
        assert_eq!(truncated.len(), 5000);
    }

    #[test]
    fn test_is_available_always_true() {
        assert!(default_provider().is_available());
    }

    #[test]
    fn test_name_is_edge() {
        assert_eq!(default_provider().name(), "edge");
    }

    #[test]
    fn test_display_name() {
        assert_eq!(default_provider().display_name(), "Microsoft Edge TTS");
    }

    #[test]
    fn test_voice_compatible_is_false() {
        assert!(!default_provider().voice_compatible());
    }
}

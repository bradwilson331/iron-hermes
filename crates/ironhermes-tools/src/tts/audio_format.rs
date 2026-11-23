// Phase 54.7 Plan 03 — audio extension / MIME helper (D-09, RESEARCH Pitfall 6)
//
// One rule for "what is this synthesized file called and what MIME type is it", shared
// by the TTS tool, the voice-test/sample endpoints and the web/Telegram/TUI sinks.
// The extension is derived from what the provider actually RETURNS, not from what a
// config field says: Edge and ElevenLabs always return MP3 bytes, so an
// `elevenlabs.output_format: opus` config must not yield a `.opus` file holding MP3.

use ironhermes_core::config::TtsConfig;
use std::path::Path;

/// Formats every audio sink (browser, Telegram, local player) accepts.
pub const SUPPORTED_SINK_FORMATS: &[&str] = &["mp3", "opus", "wav"];

/// The supported sink format matching `configured` (case-insensitive), else `"mp3"`.
pub fn effective_format(configured: &str) -> &'static str {
    let wanted = configured.trim();
    SUPPORTED_SINK_FORMATS
        .iter()
        .copied()
        .find(|f| f.eq_ignore_ascii_case(wanted))
        .unwrap_or("mp3")
}

/// File extension for audio written by the active TTS provider.
///
/// `openai` and `openai_compatible` honour their configured format (normalised to a
/// supported sink format); every other provider (edge, elevenlabs, unknown) returns
/// MP3 bytes, so the extension is `mp3`.
pub fn extension_for(tts: &TtsConfig) -> &'static str {
    extension_for_provider(tts, &tts.provider)
}

/// File extension for audio written by `provider` (a registry name), using that
/// provider's block of `tts`. Callers that may fall back to a different provider
/// than `tts.provider` (the TUI reply falls back to Edge when the configured one is
/// unavailable; `tts test --provider X`) must name the provider that ACTUALLY
/// synthesizes, or the file extension describes the wrong provider's format.
pub fn extension_for_provider(tts: &TtsConfig, provider: &str) -> &'static str {
    match provider {
        "openai" => effective_format(&tts.openai.format),
        "openai_compatible" => effective_format(&tts.openai_compatible.format),
        _ => "mp3",
    }
}

/// MIME type for an audio file, from its extension (case-insensitive).
pub fn mime_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("opus") | Some("ogg") => "audio/ogg",
        Some("wav") => "audio/wav",
        _ => "audio/mpeg",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn tts(provider: &str) -> TtsConfig {
        TtsConfig {
            provider: provider.to_string(),
            ..Default::default()
        }
    }

    /// WR-05: the extension follows the NAMED provider, not `tts.provider`.
    #[test]
    fn extension_for_provider_ignores_the_configured_provider() {
        let mut cfg = tts("openai_compatible");
        cfg.openai_compatible.format = "wav".to_string();
        assert_eq!(extension_for(&cfg), "wav");
        assert_eq!(extension_for_provider(&cfg, "edge"), "mp3");
        assert_eq!(extension_for_provider(&cfg, "elevenlabs"), "mp3");
        assert_eq!(extension_for_provider(&cfg, "openai_compatible"), "wav");
    }

    #[test]
    fn audio_format_extension_per_provider() {
        assert_eq!(extension_for(&tts("edge")), "mp3");

        let mut eleven = tts("elevenlabs");
        eleven.elevenlabs.output_format = "opus".to_string();
        assert_eq!(
            extension_for(&eleven),
            "mp3",
            "elevenlabs always returns MP3 bytes, whatever output_format says"
        );

        let mut openai = tts("openai");
        openai.openai.format = "opus".to_string();
        assert_eq!(extension_for(&openai), "opus");

        let mut compat = tts("openai_compatible");
        compat.openai_compatible.format = "wav".to_string();
        assert_eq!(extension_for(&compat), "wav");

        compat.openai_compatible.format = "flac".to_string();
        assert_eq!(extension_for(&compat), "mp3", "unsupported format -> mp3");

        assert_eq!(extension_for(&tts("some-future-provider")), "mp3");
    }

    #[test]
    fn audio_format_mime_tracks_extension() {
        assert_eq!(mime_for_path(Path::new("a.mp3")), "audio/mpeg");
        assert_eq!(mime_for_path(Path::new("a.opus")), "audio/ogg");
        assert_eq!(mime_for_path(Path::new("a.ogg")), "audio/ogg");
        assert_eq!(mime_for_path(Path::new("a.wav")), "audio/wav");
        assert_eq!(mime_for_path(Path::new("a.bin")), "audio/mpeg");
        assert_eq!(
            mime_for_path(Path::new("A.WAV")),
            "audio/wav",
            "extension match is case-insensitive"
        );
    }
}

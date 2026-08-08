// Phase 36.17.8 — STT module.
//
// This module holds:
//   1. Module declarations for the two built-in STT providers.
//   2. Re-exports for downstream consumers.
//   3. The shared `whisper_api_transcribe` helper (both providers delegate here).
//   4. The `build_stt_registry()` factory — single source of truth for wiring
//      built-in providers into a SttRegistry (mirrors build_tts_registry).
//   5. The `select_stt_provider()` helper — D-06 startup-time selection.

pub mod groq;
pub mod openai;

pub use groq::GroqSttProvider;
pub use openai::OpenAiSttProvider;

use std::path::Path;
use std::sync::Arc;

/// Shared Whisper-compatible transcription helper.
///
/// Both `GroqSttProvider` and `OpenAiSttProvider` delegate to this function —
/// the only deltas between providers are `endpoint`, `api_key`, and `model`
/// (RESEARCH "Don't Hand-Roll" pattern).
///
/// Request shape:
/// - `POST <endpoint>`
/// - `Authorization: Bearer <api_key>`
/// - `multipart/form-data`
///   - `file`: WAV bytes, `filename="audio.wav"`, `Content-Type: audio/wav`
///     (Pitfall 4: `file_name` MUST be set or the API rejects the upload)
///   - `model`: `<model>`
///   - `response_format`: `"json"`
/// - Response: `{"text": "..."}`
///
/// T-36.17.8-cost: callers are responsible for enforcing the WAV size cap
/// (`max_recording_seconds * 16000 * 2`). This helper additionally rejects
/// files exceeding the 25 MB Whisper API hard limit.
pub(crate) async fn whisper_api_transcribe(
    endpoint: &str,
    api_key: &str,
    model: &str,
    wav_path: &Path,
) -> anyhow::Result<String> {
    let bytes = tokio::fs::read(wav_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read WAV file {:?}: {}", wav_path, e))?;

    // T-36.17.8-cost: reject files exceeding the Whisper 25 MB hard limit.
    const WHISPER_MAX_BYTES: usize = 25 * 1024 * 1024;
    if bytes.len() > WHISPER_MAX_BYTES {
        return Err(anyhow::anyhow!(
            "WAV file {:?} is {} bytes, exceeding the Whisper 25 MB limit",
            wav_path,
            bytes.len()
        ));
    }

    // Build multipart form.
    // Pitfall 4: `.file_name("audio.wav")` MUST be set; without it the API
    // cannot infer the audio format and returns a 400.
    let file_part = reqwest::multipart::Part::bytes(bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| anyhow::anyhow!("Failed to set MIME type on multipart part: {}", e))?;

    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("model", model.to_owned())
        .text("response_format", "json");

    // Per-call client — mirrors web_search.rs / elevenlabs.rs pattern.
    let client = reqwest::Client::new();

    let response = client
        .post(endpoint)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Whisper API HTTP request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        // Surface full body — lets operator distinguish 401/429/503.
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Whisper API returned {}: {}", status, body));
    }

    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse Whisper API JSON response: {}", e))?;

    json["text"]
        .as_str()
        .map(|s| s.to_owned())
        .ok_or_else(|| anyhow::anyhow!("Whisper API response missing 'text' field: {:?}", json))
}

/// Build a `SttRegistry` pre-populated with the two built-in providers.
///
/// Mirrors `build_tts_registry()` from `crate::tts::mod`. This is the single
/// production call site for built-in STT provider registration.
pub fn build_stt_registry(
    config: &ironhermes_core::config::SttConfig,
) -> ironhermes_core::SttRegistry {
    let mut registry = ironhermes_core::SttRegistry::new();
    registry.register(Arc::new(GroqSttProvider::new(config.groq.clone())));
    registry.register(Arc::new(OpenAiSttProvider::new(config.openai.clone())));
    registry
}

/// D-06: Startup-time provider selection helper.
///
/// - If `config.provider != "auto"`: return `Some(config.provider.clone())` —
///   explicit configuration always wins.
/// - Else (auto mode): return `"groq"` if `GROQ_API_KEY` is set, else `"openai"`
///   if `VOICE_TOOLS_OPENAI_KEY` is set, else `None` (STT disabled).
///
/// Selection is startup-only — no mid-call failover (D-06).
pub fn select_stt_provider(config: &ironhermes_core::config::SttConfig) -> Option<String> {
    if config.provider != "auto" {
        return Some(config.provider.clone());
    }
    if std::env::var("GROQ_API_KEY").is_ok() {
        return Some("groq".to_string());
    }
    if std::env::var("VOICE_TOOLS_OPENAI_KEY").is_ok() {
        return Some("openai".to_string());
    }
    None
}

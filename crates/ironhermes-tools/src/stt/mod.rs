// Phase 36.17.8 — STT module.
//
// This module holds:
//   1. Module declarations for the two built-in STT providers.
//   2. Re-exports for downstream consumers.
//   3. The shared `whisper_api_transcribe` helper (both providers delegate here).
//   4. The `build_stt_registry()` factory — single source of truth for wiring
//      built-in providers into a SttRegistry (mirrors build_tts_registry).
//   5. The `select_stt_provider()` re-export — D-06 startup-time selection,
//      owned by `ironhermes-core` since Phase 54.7.

pub mod clip;
pub mod groq;
pub mod openai;
pub mod openai_compatible;

pub use groq::GroqSttProvider;
pub use openai::OpenAiSttProvider;
pub use openai_compatible::OpenAiCompatibleSttProvider;

use std::path::Path;
use std::sync::Arc;

/// Whisper API hard upload limit (T-36.17.8-cost, T-54.7-19).
pub(crate) const WHISPER_MAX_BYTES: usize = 25 * 1024 * 1024;

/// The multipart file name and MIME type for the audio at `path` (IN-08), from its
/// extension: `wav`, `webm`, `mp4`, `ogg`, `mp3`, `m4a`; anything else is labelled
/// WAV (the pipeline's normal output). Always a fixed literal pair, so a path
/// cannot influence what is sent as the part name.
pub(crate) fn audio_part_label(path: &Path) -> (&'static str, &'static str) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("webm") => ("audio.webm", "audio/webm"),
        Some("mp4") => ("audio.mp4", "audio/mp4"),
        Some("m4a") => ("audio.m4a", "audio/mp4"),
        Some("ogg") => ("audio.ogg", "audio/ogg"),
        Some("mp3") => ("audio.mp3", "audio/mpeg"),
        _ => ("audio.wav", "audio/wav"),
    }
}

/// Build the Whisper-shaped multipart form shared by every STT provider.
///
/// Pitfall 4: `.file_name(..)` MUST be set; without it the API cannot infer the
/// audio format and returns a 400. IN-08: the name and MIME follow the REAL
/// container (`audio_path`'s extension) -- when ffmpeg is absent the original
/// webm/mp4/ogg is passed through, and a strict server such as whisper.cpp
/// (without `--convert`) must not be told it is WAV.
pub(crate) fn whisper_form(
    bytes: Vec<u8>,
    model: &str,
    audio_path: &Path,
) -> anyhow::Result<reqwest::multipart::Form> {
    let (file_name, mime) = audio_part_label(audio_path);
    let file_part = reqwest::multipart::Part::bytes(bytes)
        .file_name(file_name)
        .mime_str(mime)
        .map_err(|e| anyhow::anyhow!("Failed to set MIME type on multipart part: {}", e))?;

    Ok(reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("model", model.to_owned())
        .text("response_format", "json"))
}

/// Extract the transcript from a Whisper-shaped JSON response.
pub(crate) fn whisper_text(json: &serde_json::Value) -> Option<String> {
    json["text"].as_str().map(|s| s.to_owned())
}

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
///   - `file`: the audio bytes, named and typed for the real container
///     (`audio.wav` / `audio/wav` for the transcoded WAV)
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
    if bytes.len() > WHISPER_MAX_BYTES {
        return Err(anyhow::anyhow!(
            "WAV file {:?} is {} bytes, exceeding the Whisper 25 MB limit",
            wav_path,
            bytes.len()
        ));
    }

    let form = whisper_form(bytes, model, wav_path)?;

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

    whisper_text(&json)
        .ok_or_else(|| anyhow::anyhow!("Whisper API response missing 'text' field: {:?}", json))
}

/// Build a `SttRegistry` pre-populated with the built-in providers.
///
/// Mirrors `build_tts_registry()` from `crate::tts::mod`. This is the single
/// production call site for built-in STT provider registration. Delegates to
/// [`build_stt_registry_with_vault`] with no vault, so a `secret/voice/*` key
/// reference on the `openai_compatible` provider does not resolve here.
pub fn build_stt_registry(
    config: &ironhermes_core::config::SttConfig,
) -> ironhermes_core::SttRegistry {
    build_stt_registry_with_vault(config, None)
}

/// Like [`build_stt_registry`], but hands the vault config to the
/// `openai_compatible` provider so a `secret/voice/*` `api_key_ref` resolves
/// (Phase 54.7 D-02).
pub fn build_stt_registry_with_vault(
    config: &ironhermes_core::config::SttConfig,
    vault: Option<ironhermes_vault::VaultConfig>,
) -> ironhermes_core::SttRegistry {
    let mut registry = ironhermes_core::SttRegistry::new();
    registry.register(Arc::new(GroqSttProvider::new(config.groq.clone())));
    registry.register(Arc::new(OpenAiSttProvider::new(config.openai.clone())));
    registry.register(Arc::new(OpenAiCompatibleSttProvider::new(
        config.openai_compatible.clone(),
        vault,
    )));
    registry
}

/// D-06: Startup-time provider selection helper.
///
/// Phase 54.7: the selection (explicit provider > openai_compatible >
/// groq > openai > none) is owned by `ironhermes-core` so the core
/// `/voice status` mirror cannot drift; this re-export keeps every existing
/// `ironhermes_tools::stt::select_stt_provider` call site compiling.
pub use ironhermes_core::stt::select_stt_provider;

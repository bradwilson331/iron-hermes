// Phase 54.7 Plan 04 — OpenAI-compatible STT provider (D-01/D-02/D-03/D-04).
//
// Targets any server speaking the OpenAI transcription multipart shape — the
// operator's whisper.cpp server is the motivating case: it 404s the default
// `/audio/transcriptions` path and answers `/inference` instead (D-03 path
// override), returning text with a leading space and trailing newline.
//
// Key handling (D-02, T-54.7-15/70): the ONLY credential source is the
// configured `api_key_ref`, resolved through the namespaced resolver
// (`VOICE_KEY_*` environment variable or `secret/voice/*` vault entry). There is
// deliberately no fallback to any built-in provider key variable, and no
// environment override of the model (Pitfall 14).
//
// Errors carry a `VoiceHttpError` category only — never a response body, a key
// or a URL with credentials (T-54.7-04).

use async_trait::async_trait;
use ironhermes_core::SttProvider;
use ironhermes_core::config::OpenAiCompatibleSttConfig;
use ironhermes_core::voice_endpoint::{STT_DEFAULT_PATH, join_endpoint};
use ironhermes_core::voice_http::{
    SPEECH_TIMEOUT, VoiceErrorCategory, VoiceHttpError, VoiceKey, build_voice_client,
    category_for_error, category_for_status, resolve_voice_key, with_optional_bearer,
};
use std::path::Path;

use crate::stt::{WHISPER_MAX_BYTES, whisper_form, whisper_text};

/// Model sent when the configured model is empty.
const FALLBACK_MODEL: &str = "whisper-1";

/// STT provider for an operator-configured OpenAI-compatible server.
pub struct OpenAiCompatibleSttProvider {
    pub config: OpenAiCompatibleSttConfig,
    vault: Option<ironhermes_vault::VaultConfig>,
}

impl OpenAiCompatibleSttProvider {
    pub fn new(
        config: OpenAiCompatibleSttConfig,
        vault: Option<ironhermes_vault::VaultConfig>,
    ) -> Self {
        Self { config, vault }
    }

    fn model(&self) -> &str {
        let m = self.config.model.trim();
        if m.is_empty() { FALLBACK_MODEL } else { m }
    }
}

fn category_error(category: VoiceErrorCategory) -> anyhow::Error {
    anyhow::Error::new(VoiceHttpError { category })
}

#[async_trait]
impl SttProvider for OpenAiCompatibleSttProvider {
    fn name(&self) -> &str {
        "openai_compatible"
    }

    fn display_name(&self) -> &str {
        "OpenAI-compatible"
    }

    /// Available exactly when a non-empty `base_url` is configured (D-01).
    fn is_available(&self) -> bool {
        !self.config.base_url.trim().is_empty()
    }

    async fn transcribe(&self, wav_path: &Path) -> anyhow::Result<String> {
        let bytes = tokio::fs::read(wav_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read WAV file {:?}: {}", wav_path, e))?;

        // T-54.7-19: same 25 MB cap as the built-in Whisper providers.
        if bytes.len() > WHISPER_MAX_BYTES {
            return Err(anyhow::anyhow!(
                "WAV file {:?} is {} bytes, exceeding the Whisper 25 MB limit",
                wav_path,
                bytes.len()
            ));
        }

        let endpoint = join_endpoint(
            &self.config.base_url,
            self.config.path.as_deref(),
            STT_DEFAULT_PATH,
        )
        .map_err(|e| anyhow::anyhow!("STT endpoint is not usable: {e}"))?;

        // D-02: reference-only key; an unresolvable reference fails before any
        // request is sent (T-54.7-15/70).
        let key = resolve_voice_key(self.config.api_key_ref.as_deref(), self.vault.as_ref()).await;
        let secret = match &key {
            VoiceKey::None => None,
            VoiceKey::Found(s) => Some(s),
            VoiceKey::NotFound => return Err(category_error(VoiceErrorCategory::KeyRejected)),
        };

        let form = whisper_form(bytes, self.model(), wav_path)?;
        let host = endpoint.host_str().unwrap_or("").to_string();

        let client = build_voice_client(SPEECH_TIMEOUT);
        let req = with_optional_bearer(client.post(endpoint).multipart(form), secret);
        let response = req.send().await.map_err(|e| {
            let category = category_for_error(&e);
            tracing::warn!(host = %host, category = category.as_str(), "openai_compatible STT request failed");
            category_error(category)
        })?;

        let status = response.status();
        if !status.is_success() {
            // D-04: log status, category and host only — never the body.
            let category = category_for_status(status);
            tracing::warn!(
                host = %host,
                status = status.as_u16(),
                category = category.as_str(),
                "openai_compatible STT returned a non-success status"
            );
            return Err(category_error(category));
        }

        let json: serde_json::Value = response.json().await.map_err(|_| {
            tracing::warn!(host = %host, "openai_compatible STT response was not JSON");
            category_error(VoiceErrorCategory::UnexpectedResponse)
        })?;

        let text = whisper_text(&json).ok_or_else(|| {
            tracing::warn!(host = %host, "openai_compatible STT response had no text field");
            category_error(VoiceErrorCategory::UnexpectedResponse)
        })?;
        Ok(text.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(base_url: &str) -> OpenAiCompatibleSttProvider {
        OpenAiCompatibleSttProvider::new(
            OpenAiCompatibleSttConfig {
                base_url: base_url.to_string(),
                ..OpenAiCompatibleSttConfig::default()
            },
            None,
        )
    }

    #[test]
    fn oc_stt_name_and_availability() {
        let empty = provider("");
        assert_eq!(empty.name(), "openai_compatible");
        assert_eq!(empty.display_name(), "OpenAI-compatible");
        assert!(!empty.is_available(), "empty base_url is unavailable");
        assert!(
            !provider("   ").is_available(),
            "whitespace base_url is unavailable"
        );
        assert!(provider("http://stt.lan:8178").is_available());
    }
}

// Phase 54.7 Plan 03 — OpenAiCompatibleTtsProvider (D-01/D-02/D-03/D-04)
//
// Synthesizes through any server speaking the OpenAI `POST {base_url}/audio/speech`
// shape (Kokoro, openedai-speech, ...). The operator's LAN and loopback hosts are
// the supported targets (D-04), so no private-address blocking is applied here.
//
// T-54.7-10: the ONLY credential consulted is the configured `api_key_ref`, resolved
//            through `resolve_voice_key` (VOICE_KEY_* env / secret/voice/* vault). This
//            file never reads a provider key variable directly.
// T-54.7-11: the client is `build_voice_client` — redirects are never followed.
// T-54.7-12: errors are `VoiceHttpError` (category string only); logs carry the status,
//            category and endpoint host — never the body, URL userinfo or any key.
// T-54.7-13: the 4096-char truncation of the OpenAI provider applies; SPEECH_TIMEOUT
//            bounds a hung server.

use crate::tts::audio_format::effective_format;
use async_trait::async_trait;
use ironhermes_core::config::OpenAiCompatibleTtsConfig;
use ironhermes_core::tts::TtsProvider;
use ironhermes_core::voice_endpoint::{TTS_DEFAULT_PATH, join_endpoint};
use ironhermes_core::voice_http::{
    AUDIO_MAX_BYTES, SPEECH_TIMEOUT, VoiceErrorCategory, VoiceHttpError, VoiceKey,
    build_voice_client, category_for_error, category_for_status, read_body_capped,
    resolve_voice_key, with_optional_bearer,
};
use std::path::{Path, PathBuf};

/// OpenAI-compatible TTS provider (`tts.provider: openai_compatible`).
pub struct OpenAiCompatibleTtsProvider {
    pub config: OpenAiCompatibleTtsConfig,
    vault: Option<ironhermes_vault::VaultConfig>,
    /// Cap on the response body (WR-10); [`AUDIO_MAX_BYTES`] unless lowered.
    max_audio_bytes: usize,
}

impl OpenAiCompatibleTtsProvider {
    /// `vault` is the config to resolve `secret/voice/*` references against; `None`
    /// means only `VOICE_KEY_*` environment references resolve (D-02).
    pub fn new(
        config: OpenAiCompatibleTtsConfig,
        vault: Option<ironhermes_vault::VaultConfig>,
    ) -> Self {
        Self {
            config,
            vault,
            max_audio_bytes: AUDIO_MAX_BYTES,
        }
    }

    /// Lower the response-body cap (never raises it above [`AUDIO_MAX_BYTES`]).
    pub fn with_max_audio_bytes(mut self, max: usize) -> Self {
        self.max_audio_bytes = max.min(AUDIO_MAX_BYTES);
        self
    }
}

fn voice_err(category: VoiceErrorCategory) -> anyhow::Error {
    anyhow::Error::new(VoiceHttpError { category })
}

#[async_trait]
impl TtsProvider for OpenAiCompatibleTtsProvider {
    fn name(&self) -> &str {
        "openai_compatible"
    }

    fn display_name(&self) -> &str {
        "OpenAI-compatible"
    }

    /// D-02: availability is not gated on a key — a keyless LAN server is valid.
    fn is_available(&self) -> bool {
        !self.config.base_url.trim().is_empty()
    }

    fn voice_compatible(&self) -> bool {
        false
    }

    async fn synthesize(&self, text: &str, output_path: &Path) -> anyhow::Result<PathBuf> {
        let text = crate::tts::openai::OpenAiTtsProvider::truncate_text(text);

        // D-03: `{base_url}` + path override, else `/audio/speech`.
        let endpoint = join_endpoint(
            &self.config.base_url,
            self.config.path.as_deref(),
            TTS_DEFAULT_PATH,
        )
        .map_err(|_| voice_err(VoiceErrorCategory::AddressNotAllowed))?;
        let host = endpoint.host_str().unwrap_or("<no host>").to_string();

        // D-02: a reference that does not resolve fails BEFORE any request is sent.
        let key = match resolve_voice_key(self.config.api_key_ref.as_deref(), self.vault.as_ref())
            .await
        {
            VoiceKey::None => None,
            VoiceKey::NotFound => {
                tracing::warn!(
                    host = %host,
                    category = VoiceErrorCategory::KeyRejected.as_str(),
                    "openai_compatible TTS key reference did not resolve; request not sent"
                );
                return Err(voice_err(VoiceErrorCategory::KeyRejected));
            }
            VoiceKey::Found(secret) => Some(secret),
        };

        let client = build_voice_client(SPEECH_TIMEOUT);
        let request = client.post(endpoint.as_str()).json(&serde_json::json!({
            "model": self.config.model,
            "voice": self.config.voice,
            "input": text,
            "response_format": effective_format(&self.config.format),
        }));
        let response = with_optional_bearer(request, key.as_ref())
            .send()
            .await
            .map_err(|e| {
                let category = category_for_error(&e);
                tracing::warn!(
                    host = %host,
                    category = category.as_str(),
                    "openai_compatible TTS request failed"
                );
                voice_err(category)
            })?;

        let status = response.status();
        if !status.is_success() {
            // Includes 3xx: redirects are not followed (D-04). The body is never
            // read into the error or the log.
            let category = category_for_status(status);
            tracing::warn!(
                host = %host,
                status = status.as_u16(),
                category = category.as_str(),
                "openai_compatible TTS server returned a non-success status"
            );
            return Err(voice_err(category));
        }

        // WR-10: stream the body under a cap. `response.bytes()` would buffer
        // whatever a misbehaving server sends (bounded only by SPEECH_TIMEOUT).
        let bytes = read_body_capped(response, self.max_audio_bytes)
            .await
            .map_err(|category| {
                tracing::warn!(
                    host = %host,
                    category = category.as_str(),
                    "openai_compatible TTS body read failed or exceeded the size cap"
                );
                voice_err(category)
            })?;
        if bytes.is_empty() {
            tracing::warn!(host = %host, "openai_compatible TTS server returned an empty body");
            return Err(voice_err(VoiceErrorCategory::UnexpectedResponse));
        }

        tokio::fs::write(output_path, &bytes)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write audio file: {e}"))?;

        Ok(output_path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(base_url: &str) -> OpenAiCompatibleTtsProvider {
        OpenAiCompatibleTtsProvider::new(
            OpenAiCompatibleTtsConfig {
                base_url: base_url.to_string(),
                ..Default::default()
            },
            None,
        )
    }

    /// Capture the `response_format` the provider sends for a configured format.
    async fn sent_response_format(format: &str) -> String {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/speech"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ID3fake".to_vec()))
            .mount(&server)
            .await;
        let p = OpenAiCompatibleTtsProvider::new(
            OpenAiCompatibleTtsConfig {
                base_url: server.uri(),
                format: format.to_string(),
                ..Default::default()
            },
            None,
        );
        let dir = tempfile::TempDir::new().unwrap();
        p.synthesize("hi", &dir.path().join("out.bin"))
            .await
            .expect("synthesize succeeds against the mock");
        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        body["response_format"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn oc_tts_sends_normalised_response_format() {
        assert_eq!(sent_response_format("flac").await, "mp3");
        assert_eq!(sent_response_format("opus").await, "opus");
        assert_eq!(sent_response_format("").await, "mp3");
    }

    /// WR-10: a body over the cap is refused while streaming, and nothing is written.
    #[tokio::test]
    async fn oc_tts_refuses_an_oversize_body_and_writes_nothing() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/speech"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 4096]))
            .mount(&server)
            .await;
        let make = |cap: usize| {
            OpenAiCompatibleTtsProvider::new(
                OpenAiCompatibleTtsConfig {
                    base_url: server.uri(),
                    ..Default::default()
                },
                None,
            )
            .with_max_audio_bytes(cap)
        };
        let dir = tempfile::TempDir::new().unwrap();

        let over = dir.path().join("over.bin");
        let err = make(1024)
            .synthesize("hi", &over)
            .await
            .expect_err("a 4096-byte body must exceed a 1024-byte cap");
        let category = err
            .downcast_ref::<VoiceHttpError>()
            .expect("a categorized voice error")
            .category;
        assert_eq!(category, VoiceErrorCategory::UnexpectedResponse);
        assert!(!over.exists(), "an oversize body must never reach disk");

        // At or above the body size the same server succeeds.
        let ok = dir.path().join("ok.bin");
        make(4096)
            .synthesize("hi", &ok)
            .await
            .expect("fits the cap");
        assert_eq!(std::fs::metadata(&ok).unwrap().len(), 4096);

        // The cap can only be lowered, never raised past the global limit.
        assert_eq!(
            make(usize::MAX).max_audio_bytes,
            AUDIO_MAX_BYTES,
            "with_max_audio_bytes clamps to AUDIO_MAX_BYTES"
        );
    }

    #[test]
    fn oc_tts_provider_name_and_availability() {
        let empty = provider("");
        assert_eq!(empty.name(), "openai_compatible");
        assert_eq!(empty.display_name(), "OpenAI-compatible");
        assert!(!empty.is_available(), "empty base_url is unavailable");
        assert!(
            !provider("   ").is_available(),
            "blank base_url is unavailable"
        );
        assert!(
            provider("http://127.0.0.1:1/v1").is_available(),
            "non-empty base_url is available without a key (D-02)"
        );
        assert!(!empty.voice_compatible());
    }
}

//! Phase 54.7 (D-10/D-12): the OpenAI-Realtime-compatible provider.
//!
//! Mint and SDP URLs are `{base_url}/realtime/client_secrets` and
//! `{base_url}/realtime/calls`, for the OpenAI default and for a
//! custom base URL alike. The HTTP client never follows redirects (D-04) and
//! never blocks LAN hosts. The permanent key is only ever a bearer on the
//! server-side mint POST; response bodies are read only to classify an error
//! and are never logged or returned (T-54.7-23).

use async_trait::async_trait;
use ironhermes_core::config::{REALTIME_DEFAULT_BASE_URL, RealtimeConfig};
use ironhermes_core::voice_endpoint::{VoiceFieldError, join_endpoint};
use ironhermes_core::voice_http::{
    DISCOVERY_MAX_BYTES, DISCOVERY_TIMEOUT, VoiceErrorCategory, build_voice_client,
    category_for_error, category_for_status, read_body_capped, with_optional_bearer,
};
use secrecy::ExposeSecret;

use super::{MintedSession, RealtimeError, RealtimeProvider};
use crate::voice_discovery::{DiscoveryOutcome, list_compat_models};

const MINT_PATH: &str = "/realtime/client_secrets";
const CALLS_PATH: &str = "/realtime/calls";

const CURATED_MODELS: &[&str] = &[
    "gpt-realtime",
    "gpt-realtime-2",
    "gpt-realtime-2.1",
    "gpt-realtime-1.5",
];
const CURATED_VOICES: &[&str] = &[
    "alloy", "ash", "ballad", "cedar", "coral", "echo", "marin", "sage", "shimmer", "verse",
];
const CURATED_TRANSCRIPTION_MODELS: &[&str] = &[
    "gpt-4o-mini-transcribe",
    "gpt-4o-transcribe",
    "whisper-1",
    "off",
];

/// The one realtime provider (id `openai_compatible`).
pub struct OpenAiCompatibleRealtime;

fn fail(category: VoiceErrorCategory) -> RealtimeError {
    RealtimeError { category }
}

#[async_trait]
impl RealtimeProvider for OpenAiCompatibleRealtime {
    fn id(&self) -> &'static str {
        "openai_compatible"
    }

    fn default_base_url(&self) -> &'static str {
        REALTIME_DEFAULT_BASE_URL
    }

    fn mint_url(&self, cfg: &RealtimeConfig) -> Result<url::Url, VoiceFieldError> {
        join_endpoint(&cfg.base_url, Some(MINT_PATH), MINT_PATH)
    }

    fn calls_url(&self, cfg: &RealtimeConfig) -> Result<url::Url, VoiceFieldError> {
        join_endpoint(&cfg.base_url, Some(CALLS_PATH), CALLS_PATH)
    }

    /// Reproduces the session object the UI server has always sent, so the
    /// call sites can swap to this without a behaviour change.
    fn build_session(
        &self,
        model: &str,
        voice: &str,
        instructions: &str,
        tools: &[serde_json::Value],
    ) -> serde_json::Value {
        serde_json::json!({
            "type": "realtime",
            "model": model,
            "instructions": instructions,
            "tools": tools,
            "tool_choice": "auto",
            "audio": { "output": { "voice": voice } }
        })
    }

    fn build_mint_request(&self, session: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "session": session })
    }

    fn should_discover_models(&self, cfg: &RealtimeConfig) -> bool {
        !cfg.is_openai_default_base_url()
    }

    async fn mint(
        &self,
        cfg: &RealtimeConfig,
        key: Option<&secrecy::SecretString>,
        body: &serde_json::Value,
    ) -> Result<MintedSession, RealtimeError> {
        // No usable key -> no request at all (D-12).
        let Some(key) = key.filter(|k| !k.expose_secret().is_empty()) else {
            return Ok(MintedSession { token: None });
        };
        let url = self
            .mint_url(cfg)
            .map_err(|_| fail(VoiceErrorCategory::AddressNotAllowed))?;
        let client = build_voice_client(DISCOVERY_TIMEOUT);
        let resp = with_optional_bearer(client.post(url), Some(key))
            .json(body)
            .send()
            .await
            .map_err(|e| fail(category_for_error(&e)))?;

        let status = resp.status();
        if status.is_success() {
            let bytes = read_body_capped(resp, DISCOVERY_MAX_BYTES)
                .await
                .map_err(fail)?;
            let json: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|_| fail(VoiceErrorCategory::UnexpectedResponse))?;
            let token = json
                .get("value")
                .and_then(|v| v.as_str())
                .or_else(|| {
                    json.get("client_secret")
                        .and_then(|c| c.get("value"))
                        .and_then(|v| v.as_str())
                })
                .filter(|t| !t.is_empty())
                .ok_or_else(|| fail(VoiceErrorCategory::UnexpectedResponse))?;
            return Ok(MintedSession {
                token: Some(token.to_string()),
            });
        }

        match status.as_u16() {
            401 | 403 => Err(fail(VoiceErrorCategory::KeyRejected)),
            400 | 404 => {
                // The body is read only to see whether the error is ABOUT the
                // model (structured fields, not free text); it is dropped
                // immediately and never logged.
                let names_model = match read_body_capped(resp, DISCOVERY_MAX_BYTES).await {
                    Ok(bytes) => error_names_model(&bytes),
                    Err(_) => false,
                };
                if names_model {
                    Err(fail(VoiceErrorCategory::ModelNotAvailable))
                } else {
                    Err(fail(category_for_status(status)))
                }
            }
            _ => Err(fail(category_for_status(status))),
        }
    }

    async fn list_models(
        &self,
        cfg: &RealtimeConfig,
        key: Option<&secrecy::SecretString>,
    ) -> DiscoveryOutcome {
        list_compat_models(&cfg.base_url, key).await
    }

    fn curated_models(&self) -> &'static [&'static str] {
        CURATED_MODELS
    }

    fn curated_voices(&self) -> &'static [&'static str] {
        CURATED_VOICES
    }

    fn curated_transcription_models(&self) -> &'static [&'static str] {
        CURATED_TRANSCRIPTION_MODELS
    }
}

/// IN-03: whether an OpenAI-style error body is about the MODEL.
///
/// When the error carries a structured `param` or `code`, ONLY those decide
/// (`param` of `model` / `*.model`, or a `code` naming a model such as
/// `model_not_found`): providers echo the request in `error.message`, so a bare
/// substring match would label an invalid voice or instruction as "model not
/// available". Only a body with neither field (a minimal server that sends just a
/// message) falls back to the message text.
pub fn error_names_model(body: &[u8]) -> bool {
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    let Some(error) = json.get("error") else {
        return false;
    };
    let param = error.get("param").and_then(|v| v.as_str());
    let code = error.get("code").and_then(|v| v.as_str());
    if param.is_some() || code.is_some() {
        let param_is_model = param.is_some_and(|p| {
            let p = p.to_ascii_lowercase();
            p == "model" || p.ends_with(".model")
        });
        let code_is_model = code.is_some_and(|c| c.to_ascii_lowercase().contains("model"));
        return param_is_model || code_is_model;
    }
    error
        .get("message")
        .and_then(|v| v.as_str())
        .is_some_and(|m| m.to_ascii_lowercase().contains("model"))
}

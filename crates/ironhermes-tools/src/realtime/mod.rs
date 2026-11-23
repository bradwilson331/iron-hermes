//! Phase 54.7 (D-10/D-12): the realtime provider abstraction.
//!
//! A [`RealtimeProvider`] knows how to build and mint a realtime session
//! against one wire protocol. Exactly ONE implementation ships this phase,
//! [`OpenAiCompatibleRealtime`]; the trait exists so the hard-coded
//! api.openai.com URLs become `{base_url}/realtime/...` and a second
//! provider can be added later without touching the call sites.

pub mod openai_compatible;

pub use openai_compatible::OpenAiCompatibleRealtime;

use async_trait::async_trait;
use ironhermes_core::config::RealtimeConfig;
use ironhermes_core::voice_endpoint::VoiceFieldError;
use ironhermes_core::voice_http::VoiceErrorCategory;

use crate::voice_discovery::DiscoveryOutcome;

/// The result of minting a realtime session.
///
/// `token` is `None` when no key was available: minting is skipped without
/// any network request (D-12) and the caller decides how to proceed.
#[derive(Clone, PartialEq, Eq)]
pub struct MintedSession {
    pub token: Option<String>,
}

impl std::fmt::Debug for MintedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shown = if self.token.is_some() {
            "Some(<redacted>)"
        } else {
            "None"
        };
        write!(f, "MintedSession {{ token: {shown} }}")
    }
}

/// A realtime failure, carrying only the fixed category (D-08).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealtimeError {
    pub category: VoiceErrorCategory,
}

impl std::fmt::Display for RealtimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.category.as_str())
    }
}

impl std::error::Error for RealtimeError {}

/// One realtime wire protocol (D-10).
#[async_trait]
pub trait RealtimeProvider: Send + Sync {
    /// Stable provider id as written in config (`realtime.provider`).
    fn id(&self) -> &'static str;
    /// The base URL used when the operator sets none.
    fn default_base_url(&self) -> &'static str;
    /// URL the server POSTs to mint an ephemeral client secret.
    fn mint_url(&self, cfg: &RealtimeConfig) -> Result<url::Url, VoiceFieldError>;
    /// URL the browser POSTs its SDP offer to.
    fn calls_url(&self, cfg: &RealtimeConfig) -> Result<url::Url, VoiceFieldError>;
    /// The realtime `session` object for the given settings.
    fn build_session(
        &self,
        model: &str,
        voice: &str,
        instructions: &str,
        tools: &[serde_json::Value],
    ) -> serde_json::Value;
    /// The full mint request body wrapping `session`.
    fn build_mint_request(&self, session: &serde_json::Value) -> serde_json::Value;
    /// True when the model list should be fetched from the server (custom
    /// base URL) rather than taken from the curated list (D-11).
    fn should_discover_models(&self, cfg: &RealtimeConfig) -> bool;
    /// Mint an ephemeral session. `key = None` makes no request (D-12).
    async fn mint(
        &self,
        cfg: &RealtimeConfig,
        key: Option<&secrecy::SecretString>,
        body: &serde_json::Value,
    ) -> Result<MintedSession, RealtimeError>;
    /// Discover the models the configured server offers.
    async fn list_models(
        &self,
        cfg: &RealtimeConfig,
        key: Option<&secrecy::SecretString>,
    ) -> DiscoveryOutcome;
    /// Curated realtime models for the default endpoint.
    fn curated_models(&self) -> &'static [&'static str];
    /// Curated realtime voices.
    fn curated_voices(&self) -> &'static [&'static str];
    /// Curated input-transcription models (including the reserved `off`).
    fn curated_transcription_models(&self) -> &'static [&'static str];
}

/// Look up a realtime provider by its config id. Exactly one exists this
/// phase (D-10); every other id, including the empty string, is `None`.
pub fn realtime_provider(id: &str) -> Option<Box<dyn RealtimeProvider>> {
    match id {
        "openai_compatible" => Some(Box::new(OpenAiCompatibleRealtime)),
        _ => None,
    }
}

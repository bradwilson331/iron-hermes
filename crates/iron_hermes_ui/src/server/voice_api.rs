//! Phase 54.7 Plan 07 (D-06/D-08/D-17): server-side helpers for the voice panel.
//!
//! Three server fns back the panel's controls:
//! - `list_voice_options` -- option-list discovery. The browser sends the
//!   unsaved address / path / key reference; the server resolves the reference
//!   and calls the target. Only `{status, items, category}` comes back (D-08).
//! - `test_stt` -- Test mic. A captured clip is transcribed with the unsaved
//!   STT settings through the production clip transcriber (D-17).
//! - `test_realtime` -- Test connection. Mints with the unsaved realtime
//!   settings and hands the browser what it needs to probe the SDP exchange.
//!
//! Each server fn is a thin wrapper: it loads `Config`, reads the write gate
//! and delegates to a `*_impl` core that tests call directly with a wiremock
//! base URL. Request/result structs are ungated (both targets compile them).
//!
//! With `security.web_config_write_enabled` off, every client-supplied URL,
//! path, key reference, model and voice is ignored and the saved config is used
//! (SSRF control, T-54.7-31). Nothing here ever writes config (D-17).

// The request/result types are only constructed by the panel (plan 08 onward)
// and by the server-side cores; until the panel calls them the wasm build sees
// them unused.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Which option list the panel wants (D-06). `family` is `tts`, `stt` or
/// `realtime`; `kind` is `models`, `voices` or (realtime only) `transcription`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct VoiceListRequest {
    pub family: String,
    pub provider: String,
    pub kind: String,
    pub base_url: Option<String>,
    pub path: Option<String>,
    pub key_ref: Option<String>,
    pub model: Option<String>,
}

/// Result of an option-list fetch. `status` is `curated`, `loaded`, `empty` or
/// `failed`; `category` is set only for `failed` and comes from the fixed
/// vocabulary. Never carries a key, header, response body or URL (D-08).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct VoiceListResult {
    pub status: String,
    pub items: Vec<String>,
    pub category: Option<String>,
}

/// Browse the option list for one picker (D-06).
#[server]
pub async fn list_voice_options(req: VoiceListRequest) -> Result<VoiceListResult, ServerFnError> {
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
    let write_enabled = config.security.web_config_write_enabled;
    list_voice_options_impl(&req, &config, write_enabled).await
}

/// Unsaved STT settings for Test mic. Every field is optional; a blank path or
/// key reference means "none", a blank model keeps the saved one (D-17).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct SttTestSettings {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub path: Option<String>,
    pub key_ref: Option<String>,
    pub model: Option<String>,
}

/// One captured clip plus the unsaved settings to transcribe it with. The clip
/// is base64 in JSON (decoded size <= 1 MiB; mime audio/webm, audio/mp4 or
/// audio/ogg).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct SttTestRequest {
    pub clip_b64: String,
    pub mime: String,
    pub settings: Option<SttTestSettings>,
}

/// Outcome of Test mic: a transcript, an `empty` flag (nothing intelligible or
/// filtered as a hallucination), or a fixed error category. Never a raw error.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct SttTestResult {
    pub transcript: Option<String>,
    pub empty: bool,
    pub category: Option<String>,
}

/// Test mic (D-17): transcribe one clip with the unsaved STT settings. Never
/// persists anything.
#[server]
pub async fn test_stt(req: SttTestRequest) -> Result<SttTestResult, ServerFnError> {
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
    let write_enabled = config.security.web_config_write_enabled;
    let cache_dir = ironhermes_core::constants::get_hermes_home().join("audio_cache");
    Ok(test_stt_impl(&req, &config, write_enabled, &cache_dir).await)
}

/// Unsaved realtime settings for Test connection. A blank address means the
/// default, a blank key reference means no key, and a blank model, voice or
/// transcription model keeps the saved one. With the write gate closed every
/// field is ignored (D-17, T-54.7-31).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct RealtimeTestSettings {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub key_ref: Option<String>,
    pub model: Option<String>,
    pub voice: Option<String>,
    pub transcription_model: Option<String>,
}

/// What the browser needs to probe a realtime server: the SDP URL, an ephemeral
/// token (None when there is no key) or the session JSON for the keyless path,
/// and a fixed error category. The permanent key never appears here (D-08).
#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct RealtimeTestTicket {
    pub calls_url: Option<String>,
    pub token: Option<String>,
    pub session_json: Option<String>,
    pub model: String,
    pub category: Option<String>,
}

// IN-01: the token is a credential and `session_json` carries the agent's
// instructions and tools -- neither may reach a `{:?}` rendering.
impl std::fmt::Debug for RealtimeTestTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealtimeTestTicket")
            .field("calls_url", &self.calls_url)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field(
                "session_json_len",
                &self.session_json.as_ref().map(String::len),
            )
            .field("model", &self.model)
            .field("category", &self.category)
            .finish()
    }
}

/// Test connection (D-17, D-12): mint with the unsaved realtime settings. The
/// browser completes and closes the session. Never persists anything.
#[server]
pub async fn test_realtime(
    settings: Option<RealtimeTestSettings>,
) -> Result<RealtimeTestTicket, ServerFnError> {
    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
    let write_enabled = config.security.web_config_write_enabled;
    Ok(test_realtime_impl(settings, &config, write_enabled).await)
}

// ── Native-only cores ───────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
use crate::voice_registry as registry;
#[cfg(not(target_arch = "wasm32"))]
use ironhermes_core::config::Config;
#[cfg(not(target_arch = "wasm32"))]
use ironhermes_core::voice_http::{resolve_voice_key, voice_vault_config, VoiceKey};
#[cfg(not(target_arch = "wasm32"))]
use ironhermes_tools::voice_discovery::DiscoveryOutcome;
#[cfg(not(target_arch = "wasm32"))]
use secrecy::SecretString;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::Semaphore;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) const STATUS_CURATED: &str = "curated";
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const STATUS_LOADED: &str = "loaded";
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const STATUS_EMPTY: &str = "empty";
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const STATUS_FAILED: &str = "failed";

/// At most four option-list fetches run at once (T-54.7-32).
#[cfg(not(target_arch = "wasm32"))]
static LIST_PERMITS: Semaphore = Semaphore::const_new(4);

/// How long a fetch waits for a free permit before reporting `timed out`.
#[cfg(not(target_arch = "wasm32"))]
const PERMIT_WAIT: Duration = Duration::from_secs(10);

/// The longest a fetch may HOLD a permit: a backstop above every per-request
/// timeout (`DISCOVERY_TIMEOUT`, 10 s) so a future endpoint added without its
/// own bound cannot pin one of the four permits.
#[cfg(not(target_arch = "wasm32"))]
const PERMIT_HOLD: Duration = Duration::from_secs(15);

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListFamily {
    Tts,
    Stt,
    Realtime,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListKind {
    Models,
    Voices,
    Transcription,
}

/// Wire error for a request the panel can never legitimately send.
#[cfg(not(target_arch = "wasm32"))]
fn list_invalid(field: &str) -> ServerFnError {
    ServerFnError::new(format!("voice_list_invalid:{field}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_family(raw: &str) -> Result<ListFamily, ServerFnError> {
    match raw {
        "tts" => Ok(ListFamily::Tts),
        "stt" => Ok(ListFamily::Stt),
        "realtime" => Ok(ListFamily::Realtime),
        _ => Err(list_invalid("family")),
    }
}

/// `models` for every family, `voices` for tts and realtime, `transcription`
/// for realtime only.
#[cfg(not(target_arch = "wasm32"))]
fn parse_kind(family: ListFamily, raw: &str) -> Result<ListKind, ServerFnError> {
    match (family, raw) {
        (_, "models") => Ok(ListKind::Models),
        (ListFamily::Tts | ListFamily::Realtime, "voices") => Ok(ListKind::Voices),
        (ListFamily::Realtime, "transcription") => Ok(ListKind::Transcription),
        _ => Err(list_invalid("kind")),
    }
}

/// The provider must be a registry id of the requested family.
#[cfg(not(target_arch = "wasm32"))]
fn registry_spec(
    family: ListFamily,
    provider: &str,
) -> Result<&'static registry::ProviderSpec, ServerFnError> {
    let spec = match family {
        ListFamily::Tts => registry::tts_provider(provider),
        ListFamily::Stt => registry::stt_provider(provider),
        ListFamily::Realtime => registry::realtime_provider(provider),
    };
    spec.ok_or_else(|| list_invalid("provider"))
}

#[cfg(not(target_arch = "wasm32"))]
fn list_result(status: &str, items: Vec<String>, category: Option<&str>) -> VoiceListResult {
    VoiceListResult {
        status: status.to_string(),
        items,
        category: category.map(str::to_string),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn failed_result(category: &str) -> VoiceListResult {
    list_result(STATUS_FAILED, Vec::new(), Some(category))
}

#[cfg(not(target_arch = "wasm32"))]
fn curated_result(items: &[&str]) -> VoiceListResult {
    list_result(
        STATUS_CURATED,
        items.iter().map(|s| s.to_string()).collect(),
        None,
    )
}

/// Map a discovery outcome onto the wire result. Ids and a fixed category are
/// the only things that survive the mapping (D-08).
#[cfg(not(target_arch = "wasm32"))]
fn outcome_result(outcome: DiscoveryOutcome) -> VoiceListResult {
    match outcome {
        DiscoveryOutcome::Loaded(ids) => list_result(STATUS_LOADED, ids, None),
        DiscoveryOutcome::Empty => list_result(STATUS_EMPTY, Vec::new(), None),
        DiscoveryOutcome::Failed(category) => failed_result(category.as_str()),
    }
}

/// The registry's curated list for one picker (D-11).
#[cfg(not(target_arch = "wasm32"))]
fn curated_for(
    spec: &registry::ProviderSpec,
    family: ListFamily,
    kind: ListKind,
) -> VoiceListResult {
    match kind {
        ListKind::Models => curated_result(spec.curated_models),
        ListKind::Voices => curated_result(spec.curated_voices),
        ListKind::Transcription => {
            debug_assert_eq!(family, ListFamily::Realtime);
            curated_result(registry::REALTIME_TRANSCRIPTION_MODELS)
        }
    }
}

/// Trim an optional string; blank becomes `None`.
#[cfg(not(target_arch = "wasm32"))]
fn non_blank(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// The address, path, key reference and model that drive one fetch.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, Default)]
struct ListInputs {
    base_url: String,
    path: Option<String>,
    key_ref: Option<String>,
    model: String,
}

/// With the write gate open the unsaved panel values are used; with it closed
/// they are all ignored and the saved config answers instead (T-54.7-31).
#[cfg(not(target_arch = "wasm32"))]
fn effective_list_inputs(
    req: &VoiceListRequest,
    config: &Config,
    family: ListFamily,
    write_enabled: bool,
) -> ListInputs {
    if write_enabled {
        return ListInputs {
            base_url: req.base_url.as_deref().unwrap_or("").trim().to_string(),
            path: non_blank(&req.path),
            key_ref: non_blank(&req.key_ref),
            model: req.model.as_deref().unwrap_or("").trim().to_string(),
        };
    }
    match family {
        ListFamily::Tts => {
            let saved = &config.tts.openai_compatible;
            ListInputs {
                base_url: saved.base_url.trim().to_string(),
                path: non_blank(&saved.path),
                key_ref: non_blank(&saved.api_key_ref),
                model: saved.model.trim().to_string(),
            }
        }
        ListFamily::Stt => {
            let saved = &config.stt.openai_compatible;
            ListInputs {
                base_url: saved.base_url.trim().to_string(),
                path: non_blank(&saved.path),
                key_ref: non_blank(&saved.api_key_ref),
                model: saved.model.trim().to_string(),
            }
        }
        ListFamily::Realtime => ListInputs {
            base_url: config.realtime.base_url.trim().to_string(),
            path: None,
            key_ref: non_blank(&config.realtime.api_key_ref),
            model: config.realtime.model().trim().to_string(),
        },
    }
}

/// Re-run the registry rules over the inputs before any request leaves the
/// server. `Err` carries the fixed category to report.
#[cfg(not(target_arch = "wasm32"))]
fn validate_list_inputs(inputs: &ListInputs, base_required: bool) -> Result<(), &'static str> {
    registry::validate_base_url(&inputs.base_url, base_required)
        .map_err(|_| registry::CATEGORY_ADDRESS_NOT_ALLOWED)?;
    if let Some(path) = &inputs.path {
        registry::validate_endpoint_path(path)
            .map_err(|_| registry::CATEGORY_ADDRESS_NOT_ALLOWED)?;
    }
    if let Some(key_ref) = &inputs.key_ref {
        registry::validate_key_ref(key_ref).map_err(|_| registry::CATEGORY_KEY_REJECTED)?;
    }
    if !inputs.model.is_empty() {
        registry::validate_model_id(&inputs.model)
            .map_err(|_| registry::CATEGORY_MODEL_NOT_AVAILABLE)?;
    }
    Ok(())
}

/// Resolve a key reference server-side. `Ok(None)` means no reference was
/// configured; `Err(())` means one was configured but cannot be resolved
/// (including any reference outside the voice namespace, T-54.7-76).
#[cfg(not(target_arch = "wasm32"))]
async fn resolve_list_key(
    config: &Config,
    key_ref: Option<&str>,
) -> Result<Option<SecretString>, ()> {
    match resolve_voice_key(key_ref, voice_vault_config(config).as_ref()).await {
        VoiceKey::None => Ok(None),
        VoiceKey::NotFound => Err(()),
        VoiceKey::Found(secret) => Ok(Some(secret)),
    }
}

/// Core of `list_voice_options`: gate substitution, validation, key
/// resolution server-side, then the discovery call (D-06/D-08).
///
/// A discovery failure is a `failed` result, never an `Err` (D-07); `Err` is
/// reserved for a request the panel cannot produce (unknown family, provider
/// or kind).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn list_voice_options_impl(
    req: &VoiceListRequest,
    config: &Config,
    write_enabled: bool,
) -> Result<VoiceListResult, ServerFnError> {
    let family = parse_family(req.family.trim())?;
    let kind = parse_kind(family, req.kind.trim())?;
    let spec = registry_spec(family, req.provider.trim())?;

    // Dispatch (D-06, D-11). Sources with a live listing endpoint are fetched;
    // everything else answers with the registry's curated list.
    let result = match (family, spec.id, kind) {
        (ListFamily::Tts, "openai_compatible", ListKind::Voices)
        | (ListFamily::Tts | ListFamily::Stt, "openai_compatible", ListKind::Models) => {
            list_compat(req, config, family, kind, write_enabled).await
        }
        (ListFamily::Tts, "elevenlabs", ListKind::Voices | ListKind::Models) => {
            list_elevenlabs(spec, kind).await
        }
        (ListFamily::Tts, "edge", ListKind::Voices) => {
            permitted(ironhermes_tools::voice_discovery::list_edge_voices()).await
        }
        (ListFamily::Realtime, "openai_compatible", ListKind::Models) => {
            list_realtime_models(spec, req, config, write_enabled).await
        }
        _ => curated_for(spec, family, kind),
    };
    Ok(result)
}

/// Run one discovery future under the shared concurrency bound (T-54.7-32).
#[cfg(not(target_arch = "wasm32"))]
async fn permitted(fetch: impl std::future::Future<Output = DiscoveryOutcome>) -> VoiceListResult {
    permitted_within(PERMIT_HOLD, fetch).await
}

/// [`permitted`] with an explicit hold limit. The permit is held for at most
/// `hold` (WR-06): a fetch with no bound of its own cannot pin a permit and starve
/// every other option list.
#[cfg(not(target_arch = "wasm32"))]
async fn permitted_within(
    hold: Duration,
    fetch: impl std::future::Future<Output = DiscoveryOutcome>,
) -> VoiceListResult {
    let Ok(Ok(_permit)) = tokio::time::timeout(PERMIT_WAIT, LIST_PERMITS.acquire()).await else {
        return failed_result(registry::CATEGORY_TIMED_OUT);
    };
    outcome_result(ironhermes_tools::voice_discovery::within_discovery_timeout(hold, fetch).await)
}

/// `openai_compatible` TTS voices / TTS models / STT models: the typed (or, with
/// the gate closed, saved) address, with the key resolved here.
#[cfg(not(target_arch = "wasm32"))]
async fn list_compat(
    req: &VoiceListRequest,
    config: &Config,
    family: ListFamily,
    kind: ListKind,
    write_enabled: bool,
) -> VoiceListResult {
    let inputs = effective_list_inputs(req, config, family, write_enabled);
    if let Err(category) = validate_list_inputs(&inputs, true) {
        return failed_result(category);
    }
    let Ok(key) = resolve_list_key(config, inputs.key_ref.as_deref()).await else {
        return failed_result(registry::CATEGORY_KEY_REJECTED);
    };
    match kind {
        ListKind::Voices => {
            permitted(ironhermes_tools::voice_discovery::list_compat_voices(
                &inputs.base_url,
                &inputs.model,
                key.as_ref(),
            ))
            .await
        }
        _ => {
            permitted(ironhermes_tools::voice_discovery::list_compat_models(
                &inputs.base_url,
                key.as_ref(),
            ))
            .await
        }
    }
}

/// ElevenLabs voices / models. The key is the `ELEVENLABS_API_KEY` environment
/// variable the TTS provider already uses; without it the curated list answers.
#[cfg(not(target_arch = "wasm32"))]
async fn list_elevenlabs(spec: &registry::ProviderSpec, kind: ListKind) -> VoiceListResult {
    use ironhermes_core::voice_http::resolve_builtin_env_key;
    use ironhermes_tools::voice_discovery::{
        list_elevenlabs_models, list_elevenlabs_voices, ELEVENLABS_API_BASE,
    };
    let VoiceKey::Found(key) = resolve_builtin_env_key("ELEVENLABS_API_KEY") else {
        return curated_for(spec, ListFamily::Tts, kind);
    };
    match kind {
        ListKind::Voices => permitted(list_elevenlabs_voices(ELEVENLABS_API_BASE, &key)).await,
        _ => permitted(list_elevenlabs_models(ELEVENLABS_API_BASE, &key)).await,
    }
}

/// Realtime models: fetched only for a custom base address (D-11), curated for
/// the api.openai.com default. The key follows the one realtime rule.
#[cfg(not(target_arch = "wasm32"))]
async fn list_realtime_models(
    spec: &registry::ProviderSpec,
    req: &VoiceListRequest,
    config: &Config,
    write_enabled: bool,
) -> VoiceListResult {
    use ironhermes_tools::realtime::realtime_provider;
    let inputs = effective_list_inputs(req, config, ListFamily::Realtime, write_enabled);
    if let Err(category) = validate_list_inputs(&inputs, false) {
        return failed_result(category);
    }
    let mut effective = config.clone();
    if !inputs.base_url.is_empty() {
        effective.realtime.base_url = inputs.base_url.clone();
    } else {
        effective.realtime.base_url = registry::REALTIME_DEFAULT_BASE_URL.to_string();
    }
    effective.realtime.api_key_ref = inputs.key_ref.clone();
    let Some(provider) = realtime_provider(&effective.realtime.provider) else {
        return failed_result(registry::CATEGORY_UNEXPECTED_RESPONSE);
    };
    if !provider.should_discover_models(&effective.realtime) {
        return curated_for(spec, ListFamily::Realtime, ListKind::Models);
    }
    let key = match crate::server::api::resolve_realtime_key(&effective).await {
        VoiceKey::None => None,
        VoiceKey::Found(secret) => Some(secret),
        VoiceKey::NotFound => return failed_result(registry::CATEGORY_KEY_REJECTED),
    };
    permitted(provider.list_models(&effective.realtime, key.as_ref())).await
}

// ── test_stt ────────────────────────────────────────────────────────────────

/// Decoded clip size cap (D-17, Open Question 7).
#[cfg(not(target_arch = "wasm32"))]
const CLIP_MAX_BYTES: usize = 1 << 20;

/// Transcript length returned to the browser.
#[cfg(not(target_arch = "wasm32"))]
const TRANSCRIPT_MAX_CHARS: usize = 2000;

/// Mime types a browser recorder produces; anything else is refused.
#[cfg(not(target_arch = "wasm32"))]
const CLIP_MIMES: [&str; 3] = ["audio/webm", "audio/mp4", "audio/ogg"];

/// One Test mic transcription at a time (T-54.7-32).
#[cfg(not(target_arch = "wasm32"))]
static STT_PERMIT: Semaphore = Semaphore::const_new(1);

#[cfg(not(target_arch = "wasm32"))]
fn stt_failed(category: &str) -> SttTestResult {
    SttTestResult {
        transcript: None,
        empty: false,
        category: Some(category.to_string()),
    }
}

/// The STT config Test mic runs with: the saved block, overlaid with the
/// validated unsaved settings only when the write gate is open (T-54.7-31).
#[cfg(not(target_arch = "wasm32"))]
fn effective_stt_config(
    config: &Config,
    settings: Option<&SttTestSettings>,
    write_enabled: bool,
) -> Result<ironhermes_core::config::SttConfig, &'static str> {
    let mut stt = config.stt.clone();
    let Some(settings) = settings.filter(|_| write_enabled) else {
        return Ok(stt);
    };
    let base_url = settings.base_url.as_deref().map(str::trim);
    let path = non_blank(&settings.path);
    let key_ref = non_blank(&settings.key_ref);
    let model = non_blank(&settings.model);

    if let Some(provider) = non_blank(&settings.provider) {
        if registry::stt_provider(&provider).is_none() {
            return Err(registry::CATEGORY_UNEXPECTED_RESPONSE);
        }
        stt.provider = provider;
    }
    if let Some(base) = base_url {
        registry::validate_base_url(base, false)
            .map_err(|_| registry::CATEGORY_ADDRESS_NOT_ALLOWED)?;
    }
    if let Some(p) = &path {
        registry::validate_endpoint_path(p).map_err(|_| registry::CATEGORY_ADDRESS_NOT_ALLOWED)?;
    }
    if let Some(k) = &key_ref {
        registry::validate_key_ref(k).map_err(|_| registry::CATEGORY_KEY_REJECTED)?;
    }
    if let Some(m) = &model {
        registry::validate_model_id(m).map_err(|_| registry::CATEGORY_MODEL_NOT_AVAILABLE)?;
    }

    if let Some(base) = base_url {
        stt.openai_compatible.base_url = base.to_string();
    }
    stt.openai_compatible.path = path;
    stt.openai_compatible.api_key_ref = key_ref;
    if let Some(model) = model {
        match stt.provider.as_str() {
            "groq" => stt.groq.model = model,
            "openai" => stt.openai.model = model,
            _ => stt.openai_compatible.model = model,
        }
    }
    Ok(stt)
}

/// Core of `test_stt` (D-17): validate the clip, transcribe it with the
/// production clip transcriber, apply the hallucination filter. Never writes
/// config and never returns a raw error.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn test_stt_impl(
    req: &SttTestRequest,
    config: &Config,
    write_enabled: bool,
    cache_dir: &std::path::Path,
) -> SttTestResult {
    use base64::Engine as _;
    use ironhermes_tools::hallucination_filter::HallucinationFilter;
    use ironhermes_tools::stt::clip::{transcribe_clip, ClipError};

    let mime_base = req
        .mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !CLIP_MIMES.contains(&mime_base.as_str()) {
        return stt_failed(registry::CATEGORY_UNEXPECTED_RESPONSE);
    }
    // Refuse an oversize payload before decoding it (4 base64 chars per 3 bytes).
    let encoded = req.clip_b64.trim();
    if encoded.len() > CLIP_MAX_BYTES.div_ceil(3) * 4 + 4 {
        return stt_failed(registry::CATEGORY_UNEXPECTED_RESPONSE);
    }
    let bytes = match base64::engine::general_purpose::STANDARD.decode(encoded) {
        Ok(bytes) if !bytes.is_empty() && bytes.len() <= CLIP_MAX_BYTES => bytes,
        _ => return stt_failed(registry::CATEGORY_UNEXPECTED_RESPONSE),
    };
    let stt = match effective_stt_config(config, req.settings.as_ref(), write_enabled) {
        Ok(stt) => stt,
        Err(category) => return stt_failed(category),
    };

    let Ok(Ok(_permit)) = tokio::time::timeout(PERMIT_WAIT, STT_PERMIT.acquire()).await else {
        return stt_failed(registry::CATEGORY_TIMED_OUT);
    };
    let vault = voice_vault_config(config);
    match transcribe_clip(&bytes, &req.mime, &stt, vault.as_ref(), cache_dir).await {
        Ok(text) => match HallucinationFilter::filter(&text) {
            Some(kept) if !kept.trim().is_empty() => SttTestResult {
                transcript: Some(kept.trim().chars().take(TRANSCRIPT_MAX_CHARS).collect()),
                empty: false,
                category: None,
            },
            _ => SttTestResult {
                transcript: None,
                empty: true,
                category: None,
            },
        },
        Err(ClipError::Transcribe(category)) => stt_failed(category.as_str()),
        Err(e @ (ClipError::NoProvider | ClipError::ProviderUnavailable(_))) => {
            stt_failed(unavailable_category(&e))
        }
        Err(ClipError::Io(_)) => stt_failed(registry::CATEGORY_UNEXPECTED_RESPONSE),
    }
}

/// IN-02: the category for "no usable STT provider". A missing key on a hosted
/// provider is a key problem; but nothing configured, or a custom server with no
/// Base URL, is not -- telling that user their key was rejected sends them to the
/// wrong field. The vocabulary is fixed, so those read `server unreachable`.
#[cfg(not(target_arch = "wasm32"))]
fn unavailable_category(error: &ironhermes_tools::stt::clip::ClipError) -> &'static str {
    match error {
        ironhermes_tools::stt::clip::ClipError::ProviderUnavailable(name)
            if name != "openai_compatible" =>
        {
            registry::CATEGORY_KEY_REJECTED
        }
        _ => registry::CATEGORY_SERVER_UNREACHABLE,
    }
}

// ── test_realtime ───────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
fn ticket_failed(model: &str, category: &str) -> RealtimeTestTicket {
    RealtimeTestTicket {
        calls_url: None,
        token: None,
        session_json: None,
        model: model.to_string(),
        category: Some(category.to_string()),
    }
}

/// The realtime config Test connection runs with: the saved block, overlaid
/// with the validated unsaved form only when the write gate is open
/// (T-54.7-31). The form is the whole picture: a blank address means the
/// default, a blank key reference means no key, and a blank model, voice or
/// transcription model keeps the saved one.
#[cfg(not(target_arch = "wasm32"))]
fn effective_realtime_config(
    config: &Config,
    settings: Option<&RealtimeTestSettings>,
    write_enabled: bool,
) -> Result<ironhermes_core::config::RealtimeConfig, &'static str> {
    let mut cfg = config.realtime.clone();
    let Some(settings) = settings.filter(|_| write_enabled) else {
        return Ok(cfg);
    };
    if let Some(provider) = non_blank(&settings.provider) {
        if registry::realtime_provider(&provider).is_none() {
            return Err(registry::CATEGORY_UNEXPECTED_RESPONSE);
        }
        cfg.provider = provider;
    }
    let base_url = settings.base_url.as_deref().unwrap_or("").trim();
    registry::validate_base_url(base_url, false)
        .map_err(|_| registry::CATEGORY_ADDRESS_NOT_ALLOWED)?;
    cfg.base_url = if base_url.is_empty() {
        registry::REALTIME_DEFAULT_BASE_URL.to_string()
    } else {
        base_url.to_string()
    };
    let key_ref = non_blank(&settings.key_ref);
    if let Some(key_ref) = &key_ref {
        registry::validate_key_ref(key_ref).map_err(|_| registry::CATEGORY_KEY_REJECTED)?;
    }
    cfg.api_key_ref = key_ref;
    if let Some(model) = non_blank(&settings.model) {
        registry::validate_model_id(&model).map_err(|_| registry::CATEGORY_MODEL_NOT_AVAILABLE)?;
        cfg.model = Some(model);
    }
    if let Some(voice) = non_blank(&settings.voice) {
        registry::validate_voice_id(&voice).map_err(|_| registry::CATEGORY_UNEXPECTED_RESPONSE)?;
        cfg.voice = Some(voice);
    }
    if let Some(transcription) = non_blank(&settings.transcription_model) {
        registry::validate_transcription_model(&transcription)
            .map_err(|_| registry::CATEGORY_MODEL_NOT_AVAILABLE)?;
        cfg.transcription_model = Some(transcription);
    }
    Ok(cfg)
}

/// Core of `test_realtime` (D-17, D-12): mint with the effective realtime
/// settings through the same provider the live session uses. No turn registry,
/// no web session row and no concurrency permit are involved, and nothing is
/// saved; the browser completes the SDP exchange and closes the session.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn test_realtime_impl(
    settings: Option<RealtimeTestSettings>,
    config: &Config,
    write_enabled: bool,
) -> RealtimeTestTicket {
    use ironhermes_tools::realtime::realtime_provider;

    let cfg = match effective_realtime_config(config, settings.as_ref(), write_enabled) {
        Ok(cfg) => cfg,
        Err(category) => return ticket_failed(config.realtime.model(), category),
    };
    let model = cfg.model().to_string();
    let Some(provider) = realtime_provider(&cfg.provider) else {
        return ticket_failed(&model, registry::CATEGORY_UNEXPECTED_RESPONSE);
    };
    let calls_url = match provider.calls_url(&cfg) {
        Ok(url) => url.to_string(),
        Err(_) => return ticket_failed(&model, registry::CATEGORY_ADDRESS_NOT_ALLOWED),
    };
    // A test session carries no instructions and no tools.
    let session = provider.build_session(cfg.model(), cfg.voice(), "", &[]);

    // The key follows the one realtime rule, applied to the effective block.
    let mut effective = config.clone();
    effective.realtime = cfg.clone();
    let key = match crate::server::api::resolve_realtime_key(&effective).await {
        VoiceKey::None => None,
        VoiceKey::Found(secret) => Some(secret),
        VoiceKey::NotFound => return ticket_failed(&model, registry::CATEGORY_KEY_REJECTED),
    };
    let body = provider.build_mint_request(&session);
    let token = match provider.mint(&cfg, key.as_ref(), &body).await {
        Ok(minted) => minted.token,
        Err(err) => return ticket_failed(&model, err.category.as_str()),
    };
    RealtimeTestTicket {
        calls_url: Some(calls_url),
        // Keyless: no token, and the browser gets the session to send itself.
        session_json: token.is_none().then(|| session.to_string()),
        token,
        model,
        category: None,
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use ironhermes_core::config::Config;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(f)
    }

    /// IN-01: Debug output never carries the token or the session JSON.
    #[test]
    fn realtime_test_ticket_debug_redacts_token_and_session() {
        let ticket = RealtimeTestTicket {
            calls_url: Some("https://rt.example/v1/realtime/calls".into()),
            token: Some("ek_FAKE_TOKEN_VALUE".into()),
            session_json: Some("{\"instructions\":\"SYSTEM PROMPT TEXT\"}".into()),
            model: "gpt-realtime".into(),
            category: None,
        };
        let shown = format!("{ticket:?}");
        assert!(!shown.contains("ek_FAKE_TOKEN_VALUE"), "{shown}");
        assert!(!shown.contains("SYSTEM PROMPT TEXT"), "{shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
        assert!(shown.contains("gpt-realtime"), "{shown}");
    }

    /// WR-06: a fetch that never answers is cut off at the hold limit, reported
    /// as `timed out`, and the permit goes back to the pool.
    #[test]
    fn stalled_fetch_releases_its_discovery_permit() {
        let before = LIST_PERMITS.available_permits();
        let result = block_on(permitted_within(
            Duration::from_millis(50),
            std::future::pending::<DiscoveryOutcome>(),
        ));
        assert_eq!(result.status, STATUS_FAILED);
        assert_eq!(result.category.as_deref(), Some(registry::CATEGORY_TIMED_OUT));
        assert_eq!(
            LIST_PERMITS.available_permits(),
            before,
            "the permit must be returned after the hold limit"
        );
    }

    /// Restores the named variables when dropped, even on panic.
    struct EnvRestore(Vec<(&'static str, Option<String>)>);

    impl EnvRestore {
        fn capture(names: &[&'static str]) -> Self {
            Self(names.iter().map(|n| (*n, std::env::var(n).ok())).collect())
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (name, value) in &self.0 {
                // SAFETY: callers hold the crate-wide env lock.
                unsafe {
                    match value {
                        Some(v) => std::env::set_var(name, v),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
    }

    fn voices_fixture() -> serde_json::Value {
        serde_json::json!({
            "object": "list",
            "model": "mlx-community/Kokoro-82M-bf16",
            "data": [
                {"id": "af_alloy", "name": "af_alloy"},
                {"id": "af_heart", "name": "af_heart"}
            ]
        })
    }

    async fn mount_voices(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/v1/audio/voices"))
            .and(query_param("model", "mlx-community/Kokoro-82M-bf16"))
            .respond_with(ResponseTemplate::new(200).set_body_json(voices_fixture()))
            .mount(server)
            .await;
    }

    fn tts_voices_request(base_url: &str) -> VoiceListRequest {
        VoiceListRequest {
            family: "tts".into(),
            provider: "openai_compatible".into(),
            kind: "voices".into(),
            base_url: Some(base_url.to_string()),
            model: Some("mlx-community/Kokoro-82M-bf16".into()),
            ..Default::default()
        }
    }

    #[test]
    fn list_compat_tts_voices_via_server_fn() {
        block_on(async {
            let server = MockServer::start().await;
            mount_voices(&server).await;
            let req = tts_voices_request(&format!("{}/v1", server.uri()));
            let res = list_voice_options_impl(&req, &Config::default(), true)
                .await
                .expect("list");
            assert_eq!(res.status, "loaded");
            assert_eq!(res.items, vec!["af_alloy", "af_heart"]);
            assert_eq!(res.category, None);
        });
    }

    #[test]
    fn list_uses_saved_values_when_writes_disabled() {
        block_on(async {
            let mock_a = MockServer::start().await;
            let mock_b = MockServer::start().await;
            mount_voices(&mock_a).await;
            mount_voices(&mock_b).await;

            let mut config = Config::default();
            config.tts.openai_compatible.base_url = format!("{}/v1", mock_b.uri());
            config.tts.openai_compatible.model = "mlx-community/Kokoro-82M-bf16".into();

            let req = tts_voices_request(&format!("{}/v1", mock_a.uri()));
            let res = list_voice_options_impl(&req, &config, false)
                .await
                .expect("list");
            assert_eq!(res.status, "loaded");
            assert_eq!(mock_a.received_requests().await.unwrap().len(), 0);
            assert_eq!(mock_b.received_requests().await.unwrap().len(), 1);
        });
    }

    #[test]
    fn list_rejects_an_invalid_base_url_without_a_request() {
        block_on(async {
            let server = MockServer::start().await;
            mount_voices(&server).await;
            let req = tts_voices_request("ftp://x");
            let res = list_voice_options_impl(&req, &Config::default(), true)
                .await
                .expect("list");
            assert_eq!(res.status, "failed");
            assert_eq!(res.category.as_deref(), Some("address not allowed"));
            assert!(res.items.is_empty());
            assert_eq!(server.received_requests().await.unwrap().len(), 0);
        });
    }

    #[test]
    fn voice_api_results_never_carry_secrets() {
        let _guard = crate::server::test_support::env_lock();
        let _restore = EnvRestore::capture(&["VOICE_KEY_IH_TEST_547_LIST"]);
        // SAFETY: the crate-wide env lock is held for the whole test.
        unsafe { std::env::set_var("VOICE_KEY_IH_TEST_547_LIST", "test-fake-key") };
        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/v1/audio/voices"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": [{"id": "af_heart", "name": "MOCK_BODY_MARKER"}],
                    "note": "MOCK_BODY_MARKER"
                })))
                .mount(&server)
                .await;
            let mut req = tts_voices_request(&format!("{}/v1", server.uri()));
            req.model = None;
            req.key_ref = Some("VOICE_KEY_IH_TEST_547_LIST".into());
            let res = list_voice_options_impl(&req, &Config::default(), true)
                .await
                .expect("list");
            assert_eq!(res.status, "loaded");

            // The key reached the target (proves it was resolved server-side) ...
            let received = server.received_requests().await.unwrap();
            let auth = received[0]
                .headers
                .get("authorization")
                .map(|v| v.to_str().unwrap_or_default().to_string())
                .unwrap_or_default();
            assert_eq!(auth, "Bearer test-fake-key");

            // ... and never comes back in the result.
            let wire = serde_json::to_string(&res).expect("serialize");
            assert!(!wire.contains("test-fake-key"), "key leaked: {wire}");
            assert!(!wire.contains("MOCK_BODY_MARKER"), "body leaked: {wire}");
        });
    }

    #[test]
    fn list_never_forwards_unrelated_key_ref() {
        let _guard = crate::server::test_support::env_lock();
        let _restore = EnvRestore::capture(&["OPENAI_API_KEY"]);
        // SAFETY: the crate-wide env lock is held for the whole test.
        unsafe { std::env::set_var("OPENAI_API_KEY", "test-fake-key") };
        block_on(async {
            let server = MockServer::start().await;
            mount_voices(&server).await;
            let mut req = tts_voices_request(&format!("{}/v1", server.uri()));
            req.key_ref = Some("OPENAI_API_KEY".into());
            let res = list_voice_options_impl(&req, &Config::default(), true)
                .await
                .expect("list");
            assert_eq!(res.status, "failed");
            assert_eq!(res.category.as_deref(), Some("key rejected"));
            assert_eq!(server.received_requests().await.unwrap().len(), 0);
        });
    }

    fn models_fixture() -> serde_json::Value {
        serde_json::json!({"data": [{"id": "tts-1"}, {"id": "kokoro"}]})
    }

    fn curated(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn list_req(
        family: &str,
        provider: &str,
        kind: &str,
        base: Option<String>,
    ) -> VoiceListRequest {
        VoiceListRequest {
            family: family.into(),
            provider: provider.into(),
            kind: kind.into(),
            base_url: base,
            ..Default::default()
        }
    }

    async fn list(req: &VoiceListRequest) -> VoiceListResult {
        list_voice_options_impl(req, &Config::default(), true)
            .await
            .expect("list")
    }

    #[test]
    fn list_dispatch_matrix() {
        let _guard = crate::server::test_support::env_lock();
        let _restore = EnvRestore::capture(&["ELEVENLABS_API_KEY"]);
        // SAFETY: the crate-wide env lock is held for the whole test.
        unsafe { std::env::remove_var("ELEVENLABS_API_KEY") };
        block_on(async {
            // tts models on an OpenAI-compatible server -> loaded.
            let tts = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/v1/models"))
                .respond_with(ResponseTemplate::new(200).set_body_json(models_fixture()))
                .mount(&tts)
                .await;
            let res = list(&list_req(
                "tts",
                "openai_compatible",
                "models",
                Some(format!("{}/v1", tts.uri())),
            ))
            .await;
            assert_eq!(res.status, "loaded");
            assert_eq!(res.items, vec!["tts-1", "kokoro"]);

            // stt models on a server with no /models endpoint (404) -> empty.
            let stt = MockServer::start().await;
            let res = list(&list_req(
                "stt",
                "openai_compatible",
                "models",
                Some(stt.uri()),
            ))
            .await;
            assert_eq!(res.status, "empty");
            assert!(res.items.is_empty());
            assert_eq!(res.category, None);

            // No discovery endpoint -> curated registry lists.
            let res = list(&list_req("stt", "groq", "models", None)).await;
            assert_eq!(res.status, "curated");
            assert_eq!(
                res.items,
                curated(&["whisper-large-v3-turbo", "whisper-large-v3"])
            );

            let res = list(&list_req("tts", "openai", "voices", None)).await;
            assert_eq!(res.status, "curated");
            assert_eq!(
                res.items,
                curated(&[
                    "alloy", "ash", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer"
                ])
            );

            let res = list(&list_req("realtime", "openai_compatible", "voices", None)).await;
            assert_eq!(res.status, "curated");
            assert_eq!(res.items.len(), 10);
            assert!(res.items.contains(&"marin".to_string()));

            let res = list(&list_req(
                "realtime",
                "openai_compatible",
                "transcription",
                None,
            ))
            .await;
            assert_eq!(res.status, "curated");
            assert_eq!(
                res.items,
                curated(&[
                    "gpt-4o-mini-transcribe",
                    "gpt-4o-transcribe",
                    "whisper-1",
                    "off"
                ])
            );

            // Realtime models on the api.openai.com default -> curated (D-11).
            let res = list(&list_req("realtime", "openai_compatible", "models", None)).await;
            assert_eq!(res.status, "curated");
            assert_eq!(res.items.len(), 4);
            let res = list(&list_req(
                "realtime",
                "openai_compatible",
                "models",
                Some("https://api.openai.com/v1/".into()),
            ))
            .await;
            assert_eq!(res.status, "curated");

            // Realtime models on a custom base -> loaded from the server.
            let rt = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/v1/models"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data": [{"id": "my-realtime"}]
                })))
                .mount(&rt)
                .await;
            let res = list(&list_req(
                "realtime",
                "openai_compatible",
                "models",
                Some(format!("{}/v1", rt.uri())),
            ))
            .await;
            assert_eq!(res.status, "loaded");
            assert_eq!(res.items, vec!["my-realtime"]);

            // ElevenLabs without a key -> curated.
            let res = list(&list_req("tts", "elevenlabs", "voices", None)).await;
            assert_eq!(res.status, "curated");
            assert_eq!(res.items, curated(&["pNInz6obpgDQGcFmaJgB"]));

            // Unknown family / provider / kind is a request the panel cannot send.
            for bad in [
                list_req("nope", "x", "models", None),
                list_req("tts", "nope", "models", None),
                list_req("stt", "groq", "voices", None),
            ] {
                assert!(list_voice_options_impl(&bad, &Config::default(), true)
                    .await
                    .is_err());
            }
        });
    }

    // ── test_stt ────────────────────────────────────────────────────────────

    fn clip_b64(len: usize) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(vec![7u8; len])
    }

    fn compat_settings(base: &str) -> SttTestSettings {
        SttTestSettings {
            provider: Some("openai_compatible".into()),
            base_url: Some(base.to_string()),
            path: Some("/inference".into()),
            model: Some("whisper-large-v3".into()),
            ..Default::default()
        }
    }

    fn stt_request(mime: &str, len: usize, settings: Option<SttTestSettings>) -> SttTestRequest {
        SttTestRequest {
            clip_b64: clip_b64(len),
            mime: mime.into(),
            settings,
        }
    }

    async fn mount_inference(server: &MockServer, text: &str) {
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": text })),
            )
            .mount(server)
            .await;
    }

    #[test]
    fn test_stt_transcribes_unsaved_compat_settings() {
        block_on(async {
            let server = MockServer::start().await;
            mount_inference(&server, " Hi.\n").await;
            let cache = tempfile::tempdir().expect("tempdir");
            let config = Config::default();
            let before = serde_yaml::to_string(&config).expect("yaml");

            let req = stt_request("audio/webm", 64, Some(compat_settings(&server.uri())));
            let res = test_stt_impl(&req, &config, true, cache.path()).await;
            assert_eq!(res.transcript.as_deref(), Some("Hi."));
            assert!(!res.empty);
            assert_eq!(res.category, None);

            assert_eq!(serde_yaml::to_string(&config).expect("yaml"), before);
            let leftover = std::fs::read_dir(cache.path()).expect("read cache").count();
            assert_eq!(leftover, 0, "temp audio must not outlive the call");
        });
    }

    #[test]
    fn test_stt_rejects_oversize_and_bad_mime() {
        block_on(async {
            let server = MockServer::start().await;
            mount_inference(&server, "Hi.").await;
            let cache = tempfile::tempdir().expect("tempdir");
            let config = Config::default();
            let settings = Some(compat_settings(&server.uri()));

            let over = stt_request("audio/webm", (1 << 20) + 1, settings.clone());
            let res = test_stt_impl(&over, &config, true, cache.path()).await;
            assert_eq!(res.category.as_deref(), Some("unexpected response"));
            assert_eq!(res.transcript, None);

            let bad = stt_request("text/plain", 16, settings.clone());
            let res = test_stt_impl(&bad, &config, true, cache.path()).await;
            assert_eq!(res.category.as_deref(), Some("unexpected response"));

            let garbled = SttTestRequest {
                clip_b64: "***not base64***".into(),
                mime: "audio/webm".into(),
                settings,
            };
            let res = test_stt_impl(&garbled, &config, true, cache.path()).await;
            assert_eq!(res.category.as_deref(), Some("unexpected response"));

            assert_eq!(server.received_requests().await.unwrap().len(), 0);
        });
    }

    #[test]
    fn test_stt_filters_hallucinations_and_reports_empty() {
        block_on(async {
            let server = MockServer::start().await;
            mount_inference(&server, "Thank you.").await;
            let cache = tempfile::tempdir().expect("tempdir");
            let req = stt_request("audio/ogg", 32, Some(compat_settings(&server.uri())));
            let res = test_stt_impl(&req, &Config::default(), true, cache.path()).await;
            assert!(res.empty);
            assert_eq!(res.transcript, None);
            assert_eq!(res.category, None);
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
        });
    }

    #[test]
    fn test_stt_maps_missing_provider_to_server_unreachable() {
        let _guard = crate::server::test_support::env_lock();
        let _restore = EnvRestore::capture(&["GROQ_API_KEY", "VOICE_TOOLS_OPENAI_KEY"]);
        // SAFETY: the crate-wide env lock is held for the whole test.
        unsafe {
            std::env::remove_var("GROQ_API_KEY");
            std::env::remove_var("VOICE_TOOLS_OPENAI_KEY");
        }
        block_on(async {
            let cache = tempfile::tempdir().expect("tempdir");
            let settings = SttTestSettings {
                provider: Some("auto".into()),
                ..Default::default()
            };
            let req = stt_request("audio/mp4", 32, Some(settings));
            let res = test_stt_impl(&req, &Config::default(), true, cache.path()).await;
            // IN-02: nothing configured is not a rejected key.
            assert_eq!(res.category.as_deref(), Some("server unreachable"));
            assert_eq!(res.transcript, None);
        });
    }

    /// IN-02: a hosted provider without its key is a key problem; a custom server
    /// with no Base URL is not.
    #[test]
    fn unavailable_provider_category_depends_on_the_provider() {
        assert_eq!(
            unavailable_category(&ironhermes_tools::stt::clip::ClipError::NoProvider),
            registry::CATEGORY_SERVER_UNREACHABLE
        );
        assert_eq!(
            unavailable_category(&ironhermes_tools::stt::clip::ClipError::ProviderUnavailable("openai_compatible".into())),
            registry::CATEGORY_SERVER_UNREACHABLE
        );
        for hosted in ["groq", "openai"] {
            assert_eq!(
                unavailable_category(&ironhermes_tools::stt::clip::ClipError::ProviderUnavailable(hosted.into())),
                registry::CATEGORY_KEY_REJECTED,
                "{hosted}"
            );
        }
    }

    // ── test_realtime ───────────────────────────────────────────────────────

    const RT_KEY_ENV: &str = "VOICE_KEY_IH_TEST_547_RT";

    fn rt_settings(base: &str, key_ref: Option<&str>) -> RealtimeTestSettings {
        RealtimeTestSettings {
            provider: Some("openai_compatible".into()),
            base_url: Some(base.to_string()),
            key_ref: key_ref.map(str::to_string),
            model: Some("gpt-realtime".into()),
            voice: Some("marin".into()),
            transcription_model: None,
        }
    }

    async fn mount_mint(server: &MockServer, response: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(path("/v1/realtime/client_secrets"))
            .respond_with(response)
            .mount(server)
            .await;
    }

    #[test]
    fn test_realtime_keyed_returns_token_and_calls_url() {
        let _guard = crate::server::test_support::env_lock();
        let _restore = EnvRestore::capture(&[RT_KEY_ENV]);
        // SAFETY: the crate-wide env lock is held for the whole test.
        unsafe { std::env::set_var(RT_KEY_ENV, "test-fake-key") };
        block_on(async {
            let server = MockServer::start().await;
            mount_mint(
                &server,
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"value": "ek_test_token"})),
            )
            .await;
            let base = format!("{}/v1", server.uri());
            let ticket = test_realtime_impl(
                Some(rt_settings(&base, Some(RT_KEY_ENV))),
                &Config::default(),
                true,
            )
            .await;
            assert_eq!(ticket.token.as_deref(), Some("ek_test_token"));
            assert_eq!(
                ticket.calls_url.as_deref(),
                Some(format!("{base}/realtime/calls").as_str())
            );
            assert_eq!(ticket.category, None);
            assert_eq!(ticket.model, "gpt-realtime");

            let received = server.received_requests().await.unwrap();
            assert_eq!(received.len(), 1);
            let body: serde_json::Value =
                serde_json::from_slice(&received[0].body).expect("mint body is JSON");
            assert_eq!(body["session"]["instructions"], "");
            assert_eq!(body["session"]["tools"], serde_json::json!([]));
            assert_eq!(body["session"]["audio"]["output"]["voice"], "marin");
            let auth = received[0]
                .headers
                .get("authorization")
                .map(|v| v.to_str().unwrap_or_default().to_string())
                .unwrap_or_default();
            assert_eq!(auth, "Bearer test-fake-key");
        });
    }

    #[test]
    fn test_realtime_keyless_returns_session_json_and_no_token() {
        block_on(async {
            let server = MockServer::start().await;
            let base = format!("{}/v1", server.uri());
            let ticket =
                test_realtime_impl(Some(rt_settings(&base, None)), &Config::default(), true).await;
            assert_eq!(ticket.token, None);
            assert_eq!(ticket.category, None);
            assert_eq!(
                ticket.calls_url.as_deref(),
                Some(format!("{base}/realtime/calls").as_str())
            );
            let session: serde_json::Value =
                serde_json::from_str(ticket.session_json.as_deref().expect("session_json"))
                    .expect("session_json parses");
            assert_eq!(session["model"], "gpt-realtime");
            assert_eq!(session["audio"]["output"]["voice"], "marin");
            assert_eq!(server.received_requests().await.unwrap().len(), 0);
        });
    }

    #[test]
    fn test_realtime_maps_mint_failures() {
        let _guard = crate::server::test_support::env_lock();
        let _restore = EnvRestore::capture(&[RT_KEY_ENV]);
        // SAFETY: the crate-wide env lock is held for the whole test.
        unsafe { std::env::set_var(RT_KEY_ENV, "test-fake-key") };
        block_on(async {
            let rejected = MockServer::start().await;
            mount_mint(
                &rejected,
                ResponseTemplate::new(401).set_body_string("nope"),
            )
            .await;
            let ticket = test_realtime_impl(
                Some(rt_settings(
                    &format!("{}/v1", rejected.uri()),
                    Some(RT_KEY_ENV),
                )),
                &Config::default(),
                true,
            )
            .await;
            assert_eq!(ticket.category.as_deref(), Some("key rejected"));
            assert_eq!(ticket.token, None);

            let no_model = MockServer::start().await;
            mount_mint(
                &no_model,
                ResponseTemplate::new(400).set_body_json(serde_json::json!({
                    "error": {"message": "The model `gpt-realtime` does not exist"}
                })),
            )
            .await;
            let ticket = test_realtime_impl(
                Some(rt_settings(
                    &format!("{}/v1", no_model.uri()),
                    Some(RT_KEY_ENV),
                )),
                &Config::default(),
                true,
            )
            .await;
            assert_eq!(ticket.category.as_deref(), Some("model not available"));

            // An invalid unsaved address never reaches the network.
            let ticket =
                test_realtime_impl(Some(rt_settings("ftp://x", None)), &Config::default(), true)
                    .await;
            assert_eq!(ticket.category.as_deref(), Some("address not allowed"));
            assert_eq!(ticket.calls_url, None);
        });
    }

    #[test]
    fn test_realtime_ignores_settings_when_writes_disabled() {
        block_on(async {
            let saved = MockServer::start().await;
            let typed = MockServer::start().await;
            let mut config = Config::default();
            config.realtime.base_url = format!("{}/v1", saved.uri());

            let ticket = test_realtime_impl(
                Some(rt_settings(&format!("{}/v1", typed.uri()), None)),
                &config,
                false,
            )
            .await;
            assert_eq!(
                ticket.calls_url.as_deref(),
                Some(format!("{}/v1/realtime/calls", saved.uri()).as_str())
            );
            assert_eq!(typed.received_requests().await.unwrap().len(), 0);
            assert_eq!(saved.received_requests().await.unwrap().len(), 0);
        });
    }

    #[test]
    fn voice_api_results_never_carry_secrets_stt_and_realtime() {
        let _guard = crate::server::test_support::env_lock();
        let _restore = EnvRestore::capture(&[RT_KEY_ENV]);
        // SAFETY: the crate-wide env lock is held for the whole test.
        unsafe { std::env::set_var(RT_KEY_ENV, "test-fake-key") };
        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(
                    ResponseTemplate::new(500).set_body_string("MOCK_BODY_MARKER test-fake-key"),
                )
                .mount(&server)
                .await;

            let cache = tempfile::tempdir().expect("tempdir");
            let mut settings = compat_settings(&server.uri());
            settings.key_ref = Some(RT_KEY_ENV.into());
            let stt = test_stt_impl(
                &stt_request("audio/webm", 16, Some(settings)),
                &Config::default(),
                true,
                cache.path(),
            )
            .await;
            let ticket = test_realtime_impl(
                Some(rt_settings(
                    &format!("{}/v1", server.uri()),
                    Some(RT_KEY_ENV),
                )),
                &Config::default(),
                true,
            )
            .await;
            assert!(stt.category.is_some() && ticket.category.is_some());
            for wire in [
                serde_json::to_string(&stt).expect("serialize"),
                serde_json::to_string(&ticket).expect("serialize"),
            ] {
                assert!(!wire.contains("test-fake-key"), "key leaked: {wire}");
                assert!(!wire.contains("MOCK_BODY_MARKER"), "body leaked: {wire}");
            }
        });
    }

    // ── client/server parity ────────────────────────────────────────────────

    fn sorted(mut ids: Vec<String>) -> Vec<String> {
        ids.sort();
        ids
    }

    fn strs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn registry_provider_ids_match_builtin_names() {
        let tts: Vec<String> = registry::TTS_PROVIDERS
            .iter()
            .map(|s| s.id.to_string())
            .collect();
        assert_eq!(
            sorted(tts),
            sorted(strs(ironhermes_core::tts::BUILTIN_TTS_NAMES))
        );

        let mut stt: Vec<String> = registry::STT_PROVIDERS
            .iter()
            .map(|s| s.id.to_string())
            .collect();
        assert_eq!(stt.iter().filter(|id| *id == "auto").count(), 1);
        stt.retain(|id| id != "auto");
        assert_eq!(
            sorted(stt),
            sorted(strs(ironhermes_core::stt::BUILTIN_STT_NAMES))
        );

        let realtime: Vec<&str> = registry::REALTIME_PROVIDERS.iter().map(|s| s.id).collect();
        assert_eq!(realtime, vec!["openai_compatible"]);
        for id in &realtime {
            assert!(ironhermes_tools::realtime::realtime_provider(id).is_some());
        }
        assert!(ironhermes_tools::realtime::realtime_provider("other").is_none());
    }

    #[test]
    fn client_and_core_validators_agree() {
        use ironhermes_core::voice_endpoint as core;

        let long_base = format!("http://{}", "a".repeat(2100));
        let long_path = format!("/{}", "a".repeat(300));
        let long_model = "a".repeat(129);
        let long_voice = "a".repeat(65);
        let long_env_ref = format!("VOICE_KEY_{}", "A".repeat(65));

        let base_urls = [
            "http://tts.lan:8880/v1",
            "https://api.openai.com/v1",
            "http://[::1]:8080/v1",
            "  http://host:9000  ",
            "ftp://x",
            "javascript:alert(1)",
            "http://user:pw@host/v1",
            "http://host/v1?x=1",
            "http://host/v1?api_key=abc",
            "http://host/v1#frag",
            "http://host/v1?",
            "not a url",
            "",
            "   ",
            long_base.as_str(),
        ];
        // An empty path means "use the default" on both sides.
        let paths = [
            "",
            "/inference",
            "/audio/speech",
            "inference",
            "/a b",
            "/a?x=1",
            "/a#f",
            long_path.as_str(),
        ];
        // An empty key reference means "no key" and is handled before either
        // validator runs, so it is not a row here.
        let key_refs = [
            "VOICE_KEY_A",
            "VOICE_KEY_MY_TTS_2",
            "secret/voice/a",
            "secret/voice/my-key.v2",
            "OPENAI_API_KEY",
            "secret/providers/openai",
            "VOICE_KEY_a",
            "VOICE_KEY_",
            "secret/voice/",
            "sk-abcdef",
            "gsk_abcdef",
            "xi-abcdef",
            "Bearer abcdef",
            "Bearerabcdef",
            "secret/voice/a..b",
            "secret/voice/../providers/x",
            "VOICE_KEY_A B",
            long_env_ref.as_str(),
        ];
        let models = [
            "gpt-realtime",
            "mlx-community/Kokoro-82M-bf16",
            "model:tag",
            "",
            "../x",
            "a..b",
            "/abs",
            "a b",
            "caf\u{e9}",
            long_model.as_str(),
        ];
        let voices = [
            "af_heart",
            "en-US-AriaNeural",
            "pNInz6obpgDQGcFmaJgB",
            "",
            "a/b",
            "a b",
            long_voice.as_str(),
        ];
        let transcription = [
            "off",
            "whisper-1",
            "gpt-4o-mini-transcribe",
            "",
            "a b",
            "OFF",
        ];

        let mut rows = 0;
        for raw in base_urls {
            rows += 1;
            assert_eq!(
                registry::validate_base_url(raw, true).is_ok(),
                core::validate_base_url(raw).is_ok(),
                "base_url row {raw:?}"
            );
        }
        for raw in paths {
            rows += 1;
            assert_eq!(
                registry::validate_endpoint_path(raw).is_ok(),
                core::validate_endpoint_path(raw).is_ok(),
                "path row {raw:?}"
            );
        }
        for raw in key_refs {
            rows += 1;
            assert_eq!(
                registry::validate_key_ref(raw).is_ok(),
                core::validate_key_ref(raw).is_ok(),
                "key_ref row {raw:?}"
            );
        }
        for raw in models {
            rows += 1;
            assert_eq!(
                registry::validate_model_id(raw).is_ok(),
                core::validate_model_id(raw).is_ok(),
                "model row {raw:?}"
            );
        }
        for raw in voices {
            rows += 1;
            assert_eq!(
                registry::validate_voice_id(raw).is_ok(),
                core::validate_voice_id(raw).is_ok(),
                "voice row {raw:?}"
            );
        }
        for raw in transcription {
            rows += 1;
            assert_eq!(
                registry::validate_transcription_model(raw).is_ok(),
                core::validate_transcription_model(raw).is_ok(),
                "transcription row {raw:?}"
            );
        }
        assert!(rows >= 30, "parity table too small: {rows}");
    }

    #[test]
    fn category_strings_match_core() {
        use ironhermes_core::voice_http::VoiceErrorCategory as C;

        // The exhaustive match makes a new core category a compile error here.
        fn all() -> [C; 6] {
            fn _exhaustive(c: C) {
                match c {
                    C::ServerUnreachable
                    | C::TimedOut
                    | C::KeyRejected
                    | C::UnexpectedResponse
                    | C::ModelNotAvailable
                    | C::AddressNotAllowed => {}
                }
            }
            [
                C::ServerUnreachable,
                C::TimedOut,
                C::KeyRejected,
                C::UnexpectedResponse,
                C::ModelNotAvailable,
                C::AddressNotAllowed,
            ]
        }
        for category in all() {
            assert!(
                registry::ERROR_CATEGORIES.contains(&category.as_str()),
                "core category {:?} missing from the registry",
                category.as_str()
            );
        }
        // The one registry-only category is a browser-side failure.
        for registry_category in registry::ERROR_CATEGORIES {
            let in_core = all().iter().any(|c| c.as_str() == registry_category);
            assert!(
                in_core || registry_category == registry::CATEGORY_BROWSER_UNREACHABLE,
                "registry category {registry_category:?} has no core counterpart"
            );
        }
    }

    #[test]
    fn realtime_curated_lists_match_tools() {
        let spec = registry::realtime_provider("openai_compatible").expect("registry entry");
        let provider =
            ironhermes_tools::realtime::realtime_provider("openai_compatible").expect("provider");
        assert_eq!(spec.curated_models, provider.curated_models());
        assert_eq!(spec.curated_voices, provider.curated_voices());
        assert_eq!(
            registry::REALTIME_TRANSCRIPTION_MODELS,
            provider.curated_transcription_models()
        );
        assert_eq!(
            registry::REALTIME_DEFAULT_BASE_URL,
            ironhermes_core::config::REALTIME_DEFAULT_BASE_URL
        );
        assert_eq!(
            spec.default_model,
            ironhermes_core::config::REALTIME_DEFAULT_MODEL
        );
        assert_eq!(
            spec.default_voice,
            ironhermes_core::config::REALTIME_DEFAULT_VOICE
        );
    }

    #[test]
    fn voice_api_has_no_config_writes() {
        // Needles are assembled from fragments so this test cannot match itself.
        let save = [".sa", "ve()"].concat();
        let save_to = [".save", "_to("].concat();
        let source = include_str!("voice_api.rs");
        assert!(!source.contains(&save), "voice_api.rs must not save config");
        assert!(
            !source.contains(&save_to),
            "voice_api.rs must not save config"
        );
    }
}

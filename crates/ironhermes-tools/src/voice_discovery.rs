//! Phase 54.7 (D-06/D-08): server-side model and voice discovery for the
//! configurable voice services.
//!
//! Every function takes the already-resolved key as `Option<&SecretString>`
//! (or `&SecretString` for ElevenLabs, which always needs one) and returns ids
//! only (D-08): no response body, header or key is ever part of a
//! [`DiscoveryOutcome`] or a log line. Requests go through the voice client,
//! which never follows redirects (D-04) and applies no private-address
//! blocking (the operator's LAN hosts are the supported targets).
//!
//! Open Question 6 resolved here: a 404 or 405 from a discovery endpoint is
//! the loaded-empty outcome, not a failure (whisper.cpp has no `/v1/models`).

use ironhermes_core::voice_endpoint::{join_endpoint, validate_model_id, validate_voice_id};
use ironhermes_core::voice_http::{
    DISCOVERY_MAX_BYTES, DISCOVERY_TIMEOUT, VoiceErrorCategory, build_voice_client,
    category_for_error, category_for_status, read_body_capped, with_optional_bearer,
};
use secrecy::ExposeSecret;

/// Maximum number of ids returned from any discovery call.
pub const MAX_ITEMS: usize = 500;

/// Maximum number of ElevenLabs voice pages followed (T-54.7-24).
const ELEVENLABS_MAX_PAGES: usize = 5;

/// Production ElevenLabs API base.
pub const ELEVENLABS_API_BASE: &str = "https://api.elevenlabs.io";

/// Outcome of a discovery call. Carries ids only (D-08): never a response
/// body, header or key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveryOutcome {
    /// The server answered with at least one usable id.
    Loaded(Vec<String>),
    /// The server answered but offers nothing (or has no listing endpoint).
    Empty,
    /// The listing failed for the given reason.
    Failed(VoiceErrorCategory),
}

/// Send a prepared discovery request and return the parsed JSON body.
///
/// The `Err` side is the finished [`DiscoveryOutcome`] to hand back: 404/405
/// is the loaded-empty outcome, every other non-2xx maps through
/// `category_for_status`, transport failures through `category_for_error`, and
/// an over-cap or non-JSON body is an unexpected response. The body is never
/// logged or returned.
async fn fetch_json(req: reqwest::RequestBuilder) -> Result<serde_json::Value, DiscoveryOutcome> {
    let resp = req
        .send()
        .await
        .map_err(|e| DiscoveryOutcome::Failed(category_for_error(&e)))?;
    let status = resp.status();
    if matches!(status.as_u16(), 404 | 405) {
        return Err(DiscoveryOutcome::Empty);
    }
    if !status.is_success() {
        return Err(DiscoveryOutcome::Failed(category_for_status(status)));
    }
    let bytes = read_body_capped(resp, DISCOVERY_MAX_BYTES)
        .await
        .map_err(DiscoveryOutcome::Failed)?;
    serde_json::from_slice(&bytes)
        .map_err(|_| DiscoveryOutcome::Failed(VoiceErrorCategory::UnexpectedResponse))
}

/// Push `id` when it passes `valid`, is not a duplicate, and the cap allows it.
fn push_id(
    out: &mut Vec<String>,
    id: &str,
    valid: fn(&str) -> Result<(), ironhermes_core::voice_endpoint::VoiceFieldError>,
) {
    if out.len() >= MAX_ITEMS || valid(id).is_err() || out.iter().any(|seen| seen == id) {
        return;
    }
    out.push(id.to_string());
}

/// Extract voice ids from the three shapes real servers return:
/// `data[].id` (OpenAI-style), `voices[].id` / `voices[].voice_id` (objects)
/// and `voices[]` (plain strings).
///
/// Each id must pass the D-11 voice-id gate; failures are dropped, duplicates
/// removed (order preserved) and at most [`MAX_ITEMS`] returned (T-54.7-21/22).
pub fn parse_voice_ids(json: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(items) = json.get("data").and_then(|d| d.as_array()) {
        for item in items {
            if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                push_id(&mut out, id, validate_voice_id);
            }
        }
    }
    if let Some(items) = json.get("voices").and_then(|d| d.as_array()) {
        for item in items {
            let id = match item {
                serde_json::Value::String(s) => Some(s.as_str()),
                _ => item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .or_else(|| item.get("voice_id").and_then(|v| v.as_str())),
            };
            if let Some(id) = id {
                push_id(&mut out, id, validate_voice_id);
            }
        }
    }
    out
}

/// Extract model ids from `data[].id`, or from a top-level array of `{id}` /
/// `{model_id}` objects. Same validation, dedupe and cap as [`parse_voice_ids`]
/// but with the D-11 model-id gate.
pub fn parse_model_ids(json: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let items = json
        .get("data")
        .and_then(|d| d.as_array())
        .or_else(|| json.as_array());
    if let Some(items) = items {
        for item in items {
            let id = item
                .get("id")
                .and_then(|v| v.as_str())
                .or_else(|| item.get("model_id").and_then(|v| v.as_str()));
            if let Some(id) = id {
                push_id(&mut out, id, validate_model_id);
            }
        }
    }
    out
}

fn outcome_from_ids(ids: Vec<String>) -> DiscoveryOutcome {
    if ids.is_empty() {
        DiscoveryOutcome::Empty
    } else {
        DiscoveryOutcome::Loaded(ids)
    }
}

/// List the models an OpenAI-compatible server offers: `GET {base}/models`.
pub async fn list_compat_models(
    base_url: &str,
    key: Option<&secrecy::SecretString>,
) -> DiscoveryOutcome {
    let url = match join_endpoint(base_url, None, "/models") {
        Ok(u) => u,
        Err(_) => return DiscoveryOutcome::Failed(VoiceErrorCategory::AddressNotAllowed),
    };
    let client = build_voice_client(DISCOVERY_TIMEOUT);
    match fetch_json(with_optional_bearer(client.get(url), key)).await {
        Ok(json) => outcome_from_ids(parse_model_ids(&json)),
        Err(outcome) => outcome,
    }
}

/// List the voices an OpenAI-compatible TTS server offers for `model`.
///
/// Sends `GET {base}/audio/voices?model=<model>`; the model is appended with
/// `query_pairs_mut` so it is percent-encoded, never string-formatted into the
/// URL. An empty `model` sends no query.
pub async fn list_compat_voices(
    base_url: &str,
    model: &str,
    key: Option<&secrecy::SecretString>,
) -> DiscoveryOutcome {
    let mut url = match join_endpoint(base_url, None, "/audio/voices") {
        Ok(u) => u,
        Err(_) => return DiscoveryOutcome::Failed(VoiceErrorCategory::AddressNotAllowed),
    };
    if !model.is_empty() {
        url.query_pairs_mut().append_pair("model", model);
    }
    let client = build_voice_client(DISCOVERY_TIMEOUT);
    match fetch_json(with_optional_bearer(client.get(url), key)).await {
        Ok(json) => outcome_from_ids(parse_voice_ids(&json)),
        Err(outcome) => outcome,
    }
}

/// Attach the ElevenLabs `xi-api-key` header. A key that is empty or holds a
/// control character cannot be sent safely and is reported as rejected.
fn elevenlabs_request(
    client: &reqwest::Client,
    url: url::Url,
    key: &secrecy::SecretString,
) -> Result<reqwest::RequestBuilder, DiscoveryOutcome> {
    let value = key.expose_secret();
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(DiscoveryOutcome::Failed(VoiceErrorCategory::KeyRejected));
    }
    Ok(client.get(url).header("xi-api-key", value))
}

/// List ElevenLabs voices: `GET {api_base}/v2/voices?page_size=100`, following
/// `next_page_token` while `has_more` is true, at most 5 pages. A failure on a
/// later page keeps the ids already collected.
pub async fn list_elevenlabs_voices(
    api_base: &str,
    key: &secrecy::SecretString,
) -> DiscoveryOutcome {
    let base_url = match join_endpoint(api_base, None, "/v2/voices") {
        Ok(u) => u,
        Err(_) => return DiscoveryOutcome::Failed(VoiceErrorCategory::AddressNotAllowed),
    };
    let client = build_voice_client(DISCOVERY_TIMEOUT);
    let mut collected: Vec<String> = Vec::new();
    let mut page_token: Option<String> = None;

    for page in 0..ELEVENLABS_MAX_PAGES {
        let mut url = base_url.clone();
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("page_size", "100");
            if let Some(tok) = page_token.as_deref() {
                q.append_pair("next_page_token", tok);
            }
        }
        let req = match elevenlabs_request(&client, url, key) {
            Ok(r) => r,
            Err(outcome) => return outcome,
        };
        let json = match fetch_json(req).await {
            Ok(j) => j,
            Err(outcome) if page == 0 => return outcome,
            Err(_) => break,
        };
        for id in parse_voice_ids(&json) {
            push_id(&mut collected, &id, validate_voice_id);
        }
        let has_more = json
            .get("has_more")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        page_token = json
            .get("next_page_token")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if !has_more || page_token.is_none() || collected.len() >= MAX_ITEMS {
            break;
        }
    }
    outcome_from_ids(collected)
}

/// List ElevenLabs models: `GET {api_base}/v1/models`, keeping entries whose
/// `can_do_text_to_speech` is not `false`; the id is `model_id`.
pub async fn list_elevenlabs_models(
    api_base: &str,
    key: &secrecy::SecretString,
) -> DiscoveryOutcome {
    let url = match join_endpoint(api_base, None, "/v1/models") {
        Ok(u) => u,
        Err(_) => return DiscoveryOutcome::Failed(VoiceErrorCategory::AddressNotAllowed),
    };
    let client = build_voice_client(DISCOVERY_TIMEOUT);
    let req = match elevenlabs_request(&client, url, key) {
        Ok(r) => r,
        Err(outcome) => return outcome,
    };
    let json = match fetch_json(req).await {
        Ok(j) => j,
        Err(outcome) => return outcome,
    };
    let mut out: Vec<String> = Vec::new();
    if let Some(items) = json.as_array() {
        for item in items {
            let tts_capable = item
                .get("can_do_text_to_speech")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if !tts_capable {
                continue;
            }
            if let Some(id) = item.get("model_id").and_then(|v| v.as_str()) {
                push_id(&mut out, id, validate_model_id);
            }
        }
    }
    outcome_from_ids(out)
}

/// Map Edge voice `short_name` values to ids: missing names and names that
/// fail the voice-id gate are dropped, duplicates removed, capped.
pub fn edge_voice_ids(short_names: impl IntoIterator<Item = Option<String>>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in short_names.into_iter().flatten() {
        push_id(&mut out, &name, validate_voice_id);
    }
    out
}

/// List the Edge TTS voices via `msedge_tts`. Any error is reported as
/// `ServerUnreachable`; the error text is never surfaced.
///
/// WR-06: `msedge_tts` builds its own `reqwest::Client::new()` (no timeout), so
/// the call is bounded here by [`DISCOVERY_TIMEOUT`]; a stalled route to Microsoft
/// reports `TimedOut` instead of holding a discovery permit indefinitely.
pub async fn list_edge_voices() -> DiscoveryOutcome {
    // msedge-tts uses rustls with aws-lc-rs; the provider must be installed
    // once per process before the first TLS connection (already-installed is fine).
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    within_discovery_timeout(DISCOVERY_TIMEOUT, async {
        match msedge_tts::voice::tokio_runtime::get_voices_list_async().await {
            Ok(voices) => {
                outcome_from_ids(edge_voice_ids(voices.into_iter().map(|v| v.short_name)))
            }
            Err(_) => DiscoveryOutcome::Failed(VoiceErrorCategory::ServerUnreachable),
        }
    })
    .await
}

/// Run one discovery future for at most `limit`; an overrun is reported as
/// `TimedOut` and the future is dropped (cancelling the request and releasing
/// whatever the caller was holding for it).
pub async fn within_discovery_timeout(
    limit: std::time::Duration,
    fetch: impl std::future::Future<Output = DiscoveryOutcome>,
) -> DiscoveryOutcome {
    tokio::time::timeout(limit, fetch)
        .await
        .unwrap_or(DiscoveryOutcome::Failed(VoiceErrorCategory::TimedOut))
}

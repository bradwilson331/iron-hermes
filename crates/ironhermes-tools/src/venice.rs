//! Phase 47 Plan 02 — Venice.ai HTTP client (`VeniceClient`).
//!
//! Venice is the new default backend for image/video generation (GEN-02). Its
//! wire protocol is meaningfully different from `fal.rs`'s fal.ai queue API:
//!
//! - **Image generation is synchronous**: a single `POST /image/generate`
//!   returns the finished image inline as base64 (`images[0]`) — there is no
//!   submit/poll wrapper and no CDN URL to SSRF-check (RESEARCH Pitfall 1).
//! - **Video generation is a 2-endpoint async lifecycle**: `POST /video/queue`
//!   returns `{model, queue_id}`, then a loop of `POST /video/retrieve` that
//!   echoes BOTH `model` and `queue_id` on every call (Pitfall 2) until the
//!   response is `COMPLETED` (raw `video/mp4` binary for public models, or a
//!   JSON envelope carrying a `download_url` for private/VPS-backed models).
//! - **There is no `FAILED` status.** Venice's `/video/retrieve` schema only
//!   enumerates `PROCESSING`/`COMPLETED`; any failure surfaces as a non-2xx
//!   HTTP response (Pitfall 3). A never-completing job resolves to a distinct
//!   timeout error (GEN-02) — never a silent empty result, never a hang.
//! - **`model_spec.constraints` casing differs by media type**: image
//!   constraints are camelCase, video constraints are snake_case — genuinely
//!   two different schema branches (Pitfall 4), modeled as two Rust structs.
//!
//! The `VENICE_API_KEY` is passed per-call and never stored on the client, so
//! the derived `Debug` impl is safe (mirrors `FalClient`'s SAFE-02 pattern) —
//! there is nothing on the struct to redact.

use std::time::Duration;

use base64::Engine as _;

/// Default Venice API base URL (overridable for tests).
const DEFAULT_BASE_URL: &str = "https://api.venice.ai/api/v1";

/// Maximum body excerpt length surfaced in a terminal error (never the key).
const ERROR_BODY_EXCERPT_LEN: usize = 500;

/// Lazy-key Venice API client. Holds only the base URL — never the key.
///
/// `Debug` is derived safely: the struct carries no secret (`VENICE_API_KEY`
/// is passed per call and never stored), so there is nothing to redact.
#[derive(Debug, Clone)]
pub struct VeniceClient {
    base_url: String,
}

impl Default for VeniceClient {
    fn default() -> Self {
        Self::new()
    }
}

impl VeniceClient {
    /// Construct a client targeting the production Venice API base URL.
    pub fn new() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    /// Construct a client with an injected base URL (test-only entry point —
    /// lets `wiremock`'s `server.uri()` stand in for `api.venice.ai`).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    /// The configured API base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    // -----------------------------------------------------------------
    // Image generation — synchronous, base64 (Pitfall 1)
    // -----------------------------------------------------------------

    /// Generate an image via a SINGLE synchronous `POST /image/generate` and
    /// decode the base64 `images[0]` payload to raw bytes.
    ///
    /// There is no poll wrapper, no CDN URL, no SSRF check on the output —
    /// Venice's image response carries the finished bytes inline (Pitfall 1).
    /// Any non-2xx response is a terminal error (never a silent empty `Ok`).
    /// `format` is one of `jpeg`/`png`/`webp` (Venice default `webp`); `width`
    /// / `height` are optional dimension overrides.
    ///
    /// `steps` is the diffusion sampling-step count: it MUST be sent, because
    /// Venice does not apply the model's own `constraints.steps.default` when the
    /// field is omitted — an omitted/`0` `steps` runs a near-zero denoise pass and
    /// returns an under-denoised "blob" image, not a finished frame. A `Some(n)`
    /// with `n > 0` is forwarded verbatim (callers clamp to the model's `max`);
    /// `None`/`Some(0)` falls back to Venice's server-side handling. `safe_mode`
    /// (Venice blurs adult-flagged output when `true`) is caller-controlled rather
    /// than hardcoded, so operators can disable the blur for a single-user host.
    #[allow(clippy::too_many_arguments)]
    pub async fn generate_image(
        &self,
        api_key: &str,
        model: &str,
        prompt: &str,
        format: &str,
        width: Option<u32>,
        height: Option<u32>,
        steps: Option<u32>,
        safe_mode: bool,
    ) -> anyhow::Result<Vec<u8>> {
        let client = reqwest::Client::new();
        let url = format!("{}/image/generate", self.base_url);

        let mut body = serde_json::json!({
            "model": model,
            "prompt": prompt,
            "format": format,
            "return_binary": false,
            "variants": 1,
            "safe_mode": safe_mode,
        });
        if let (Some(w), Some(h)) = (width, height) {
            body["width"] = serde_json::json!(w);
            body["height"] = serde_json::json!(h);
        }
        // A real denoise pass needs an explicit step count — omitting it (or 0)
        // yields blob output (see method doc). Only send a positive value.
        if let Some(s) = steps.filter(|s| *s > 0) {
            body["steps"] = serde_json::json!(s);
        }

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("venice image request failed: {}", e))?;

        let status = response.status();
        if !status.is_success() {
            let excerpt = body_excerpt(response).await;
            anyhow::bail!("venice image generation returned {}: {}", status, excerpt);
        }

        let decoded: ImageGenerateResponse = response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("venice image response decode failed: {}", e))?;

        let first = decoded
            .images
            .first()
            .ok_or_else(|| anyhow::anyhow!("venice image response contained no images"))?;

        base64::engine::general_purpose::STANDARD
            .decode(first)
            .map_err(|e| anyhow::anyhow!("venice image base64 decode failed: {}", e))
    }

    // -----------------------------------------------------------------
    // Video generation — 2-endpoint async lifecycle (Pattern 3)
    // -----------------------------------------------------------------

    /// Submit a video generation job: `POST /video/queue`.
    ///
    /// `body` carries `model`/`prompt`/`duration`/`resolution`/`aspect_ratio`
    /// plus mode-specific fields (`image_url` for i2v, `video_url` for v2v)
    /// supplied by the caller — this method does not shape the request body
    /// itself, since the fields vary by mode (t2v/i2v/v2v). Non-2xx is a
    /// terminal error (no FAILED status exists — Pitfall 3).
    pub async fn queue_video(
        &self,
        api_key: &str,
        body: serde_json::Value,
    ) -> anyhow::Result<QueuedVideoJob> {
        let client = reqwest::Client::new();
        let url = format!("{}/video/queue", self.base_url);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("venice video queue request failed: {}", e))?;

        let status = response.status();
        if !status.is_success() {
            let excerpt = body_excerpt(response).await;
            anyhow::bail!("venice video queue returned {}: {}", status, excerpt);
        }

        response
            .json::<QueuedVideoJob>()
            .await
            .map_err(|e| anyhow::anyhow!("venice video queue response decode failed: {}", e))
    }

    /// Poll `POST /video/retrieve` (re-sending BOTH `model` and `queue_id` on
    /// every call — Pitfall 2) until the job completes or `timeout` elapses.
    ///
    /// Branches on the response `Content-Type`:
    /// - `video/mp4` (or any `video/*`) → the raw body IS the finished bytes
    ///   (public models).
    /// - `application/json` with `status: "PROCESSING"` → keep polling.
    /// - `application/json` with `status: "COMPLETED"` and a `download_url`
    ///   → an authless `GET` on `download_url` fetches the bytes
    ///   (private/VPS-backed models).
    ///
    /// Any non-2xx response at any point is a terminal, non-retried error —
    /// Venice has no `FAILED` status and no cancel endpoint, so a timeout
    /// simply stops polling and returns a distinct timeout error (never a
    /// fal-style cancel `PUT`). `on_tick` fires once per poll iteration (the
    /// config-gated progress-ping hook consumed by Plan 06).
    pub async fn retrieve_video(
        &self,
        api_key: &str,
        job: &QueuedVideoJob,
        timeout: Duration,
        on_tick: Option<&(dyn Fn() + Send + Sync)>,
    ) -> anyhow::Result<Vec<u8>> {
        let client = reqwest::Client::new();
        let url = format!("{}/video/retrieve", self.base_url);
        let deadline = tokio::time::Instant::now() + timeout;
        let mut delay = Duration::from_millis(500);

        loop {
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("venice video generation timed out after {:?}", timeout);
            }

            if let Some(tick) = on_tick {
                tick();
            }

            let response = client
                .post(&url)
                .header("Authorization", format!("Bearer {api_key}"))
                .json(&serde_json::json!({
                    "model": job.model,
                    "queue_id": job.queue_id,
                    "delete_media_on_completion": false,
                }))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("venice video retrieve request failed: {}", e))?;

            let status = response.status();
            if !status.is_success() {
                let excerpt = body_excerpt(response).await;
                anyhow::bail!("venice video retrieve returned {}: {}", status, excerpt);
            }

            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();

            if content_type.starts_with("video/") {
                let bytes = response.bytes().await.map_err(|e| {
                    anyhow::anyhow!("venice video retrieve body read failed: {}", e)
                })?;
                return Ok(bytes.to_vec());
            }

            // application/json (or unlabeled) — parse the status envelope.
            let envelope: RetrieveStatusResponse = response.json().await.map_err(|e| {
                anyhow::anyhow!("venice video retrieve status decode failed: {}", e)
            })?;

            match envelope.status.as_str() {
                "COMPLETED" => {
                    let download_url = envelope.download_url.ok_or_else(|| {
                        anyhow::anyhow!(
                            "venice video retrieve reported COMPLETED with no binary body and no download_url"
                        )
                    })?;
                    // Authless GET (24h pre-signed URL — RESEARCH Code Examples).
                    let dl_response = client
                        .get(&download_url)
                        .send()
                        .await
                        .map_err(|e| {
                            anyhow::anyhow!("venice video download request failed: {}", e)
                        })?
                        .error_for_status()
                        .map_err(|e| anyhow::anyhow!("venice video download failed: {}", e))?;
                    let bytes = dl_response.bytes().await.map_err(|e| {
                        anyhow::anyhow!("venice video download body read failed: {}", e)
                    })?;
                    return Ok(bytes.to_vec());
                }
                "PROCESSING" => {
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(5));
                }
                other => {
                    // No FAILED status exists in Venice's schema (Pitfall 3) —
                    // an unrecognized status string is still not a failure
                    // signal we should invent; keep polling defensively rather
                    // than bailing on a forward-compatible new status value,
                    // but never loop forever (the timeout check above bounds
                    // this).
                    tracing::debug!(status = %other, "venice video retrieve: unrecognized status, continuing to poll");
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(5));
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // model_spec.constraints — dual casing (Pitfall 4) + D-13 validator
    // -----------------------------------------------------------------

    /// `GET /models?type=image|video` — fetches the model catalog for one
    /// media type and returns each entry's id plus its typed constraints
    /// branch (image → camelCase, video → snake_case; never mixed).
    pub async fn fetch_models(
        &self,
        api_key: &str,
        media_type: &str,
    ) -> anyhow::Result<Vec<ModelEntry>> {
        let client = reqwest::Client::new();
        let url = format!("{}/models?type={}", self.base_url, media_type);

        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("venice models request failed: {}", e))?;

        let status = response.status();
        if !status.is_success() {
            let excerpt = body_excerpt(response).await;
            anyhow::bail!("venice models list returned {}: {}", status, excerpt);
        }

        let list: ModelsListResponse = response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("venice models response decode failed: {}", e))?;

        let mut out = Vec::with_capacity(list.data.len());
        for raw in list.data {
            let constraints_value = raw
                .model_spec
                .map(|spec| spec.constraints)
                .unwrap_or(serde_json::Value::Null);
            let constraints = if media_type == "video" {
                let parsed: VideoModelConstraints =
                    serde_json::from_value(constraints_value).unwrap_or_default();
                ModelConstraints::Video(parsed)
            } else {
                let parsed: ImageModelConstraints =
                    serde_json::from_value(constraints_value).unwrap_or_default();
                ModelConstraints::Image(parsed)
            };
            out.push(ModelEntry {
                id: raw.id,
                constraints,
            });
        }
        Ok(out)
    }

    /// Resolve the configured integer `duration_secs` against a video model's
    /// allowed duration strings from the live catalog (D-13). Venice expresses
    /// durations as `"{n}s"` (e.g. `"5s"`) — or the literal `"Auto"` for models
    /// that determine duration themselves (video-to-video, upscale,
    /// motion-control) — NEVER a bare integer, which is why stringifying the
    /// `u32` alone always mismatched the catalog.
    ///
    /// - `Ok(Some(v))` — the exact string to send; it is in the model's set.
    /// - `Ok(None)`    — the model declares NO duration constraint (empty list),
    ///   so `duration` must be omitted from the request (the model decides).
    /// - `Err(msg)`    — the configured seconds is not one this model offers,
    ///   naming the exact allowed values so the fix is unambiguous.
    pub fn resolve_video_duration(
        secs: u32,
        allowed: &[String],
        model: &str,
        config_path: &str,
    ) -> Result<Option<String>, String> {
        if allowed.is_empty() {
            return Ok(None);
        }
        let want = format!("{secs}s");
        if allowed.iter().any(|a| a == &want) {
            return Ok(Some(want));
        }
        // Models whose duration is model-determined expose exactly `["Auto"]`
        // (video-to-video, upscale, motion-control) — the configured seconds is
        // irrelevant there, so honour the model's single option.
        if let Some(auto) = allowed.iter().find(|a| a.eq_ignore_ascii_case("auto")) {
            return Ok(Some(auto.clone()));
        }
        Err(format!(
            "duration {want} not supported by {model}; allowed: {}. Fix {config_path}.",
            allowed.join(", ")
        ))
    }

    /// Resolve a configured string param (`resolution` / `aspect_ratio`) against
    /// a video model's allowed list (D-13). An EMPTY allowed list means the
    /// model does not accept this param at all — every image-to-video model
    /// returns `aspect_ratios: []` because it derives the ratio from the input
    /// image, and many models return `resolutions: []` — so an empty list yields
    /// `Ok(None)` (omit the param), NOT a rejection. A non-empty list is matched
    /// exactly; a value outside it is `Err` naming the allowed set.
    ///
    /// The fail-closed guard against an *uncatalogued* model does NOT live here:
    /// callers MUST reject a model absent from the catalog before resolving, so
    /// an unknown model can never reach this "empty ⇒ omit" path.
    pub fn resolve_video_param(
        param: &str,
        value: &str,
        allowed: &[String],
        model: &str,
        config_path: &str,
    ) -> Result<Option<String>, String> {
        if allowed.is_empty() {
            return Ok(None);
        }
        if allowed.iter().any(|a| a == value) {
            Ok(Some(value.to_string()))
        } else {
            Err(format!(
                "{param} {value} not supported by {model}; allowed: {}. Fix {config_path}.",
                allowed.join(", ")
            ))
        }
    }
}

/// Fail-closed model lookup for the video path (D-13): find `model`'s entry in
/// a freshly-fetched catalog and return its video constraints. An uncatalogued
/// model — or one whose entry is an image model — is a hard error naming the
/// config key to fix, so a typo can never reach the "empty ⇒ omit" resolvers
/// ([`VeniceClient::resolve_video_duration`] /
/// [`VeniceClient::resolve_video_param`]) as an unconstrained request.
pub fn video_constraints_for_model(
    entries: Vec<ModelEntry>,
    model: &str,
    config_path_prefix: &str,
) -> anyhow::Result<VideoModelConstraints> {
    let entry = entries.into_iter().find(|e| e.id == model).ok_or_else(|| {
        anyhow::anyhow!(
            "Venice has no video model '{model}'. Set {config_path_prefix}.model to a \
             catalogued id (e.g. wan-2-7-text-to-video)."
        )
    })?;
    match entry.constraints {
        ModelConstraints::Video(c) => Ok(c),
        ModelConstraints::Image(_) => Err(anyhow::anyhow!(
            "'{model}' is an image model, not a video model. Set {config_path_prefix}.model \
             to a video model."
        )),
    }
}

/// Read the response body (best-effort) and truncate it to a bounded excerpt
/// for a terminal error message. Never includes the `Authorization` header —
/// callers only ever pass the response body here.
async fn body_excerpt(response: reqwest::Response) -> String {
    let text = response.text().await.unwrap_or_default();
    if text.len() > ERROR_BODY_EXCERPT_LEN {
        // Truncate on a UTF-8 char boundary — a raw byte slice at
        // ERROR_BODY_EXCERPT_LEN panics when a multi-byte char straddles the
        // cut (CR-01). Matches the is_char_boundary walk in delegate_task.rs.
        let mut end = ERROR_BODY_EXCERPT_LEN;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &text[..end])
    } else {
        text
    }
}

/// `POST /image/generate` response envelope (`return_binary: false`).
/// `images[0]` is base64-encoded bytes — NOT a URL (Pitfall 1).
#[derive(Debug, serde::Deserialize)]
struct ImageGenerateResponse {
    #[serde(default)]
    images: Vec<String>,
}

/// `POST /video/queue` response: `{model, queue_id, download_url?}`.
///
/// Carries `model` + `queue_id` + an optional `download_url` (present only
/// for private/VPS-backed models) so the retrieve loop can re-send both IDs
/// on every poll (Pitfall 2).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct QueuedVideoJob {
    pub model: String,
    pub queue_id: String,
    #[serde(default)]
    pub download_url: Option<String>,
}

/// `POST /video/retrieve` JSON envelope (PROCESSING or COMPLETED-without-binary).
///
/// There is no `FAILED` variant in Venice's schema (Pitfall 3) — failures are
/// non-2xx HTTP responses instead, handled before this struct is ever parsed.
#[derive(Debug, serde::Deserialize)]
struct RetrieveStatusResponse {
    #[serde(default)]
    status: String,
    #[serde(default)]
    download_url: Option<String>,
}

/// Raw `GET /models` list envelope (`{ "data": [...] }`).
#[derive(Debug, serde::Deserialize)]
struct ModelsListResponse {
    #[serde(default)]
    data: Vec<RawModelEntry>,
}

/// Raw model catalog entry before constraints are typed by media type.
#[derive(Debug, serde::Deserialize)]
struct RawModelEntry {
    id: String,
    #[serde(default)]
    model_spec: Option<RawModelSpec>,
}

/// Raw `model_spec` wrapper — `constraints` is parsed generically here and
/// re-typed as [`ImageModelConstraints`] or [`VideoModelConstraints`]
/// depending on the requested media type (Pitfall 4: these are two distinct
/// schema branches, never unified into one struct).
#[derive(Debug, serde::Deserialize)]
struct RawModelSpec {
    #[serde(default)]
    constraints: serde_json::Value,
}

/// A fetched Venice model catalog entry: its id plus the typed constraints
/// branch matching the media type it was fetched under.
#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub id: String,
    pub constraints: ModelConstraints,
}

/// Typed `model_spec.constraints` — one of two genuinely different schema
/// branches (Pitfall 4), never a single shared shape.
#[derive(Debug, Clone)]
pub enum ModelConstraints {
    Image(ImageModelConstraints),
    Video(VideoModelConstraints),
}

/// `steps` sub-object of [`ImageModelConstraints`]: `{ "default": N, "max": N }`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct StepsConstraint {
    #[serde(default)]
    pub default: u32,
    #[serde(default)]
    pub max: u32,
}

/// Image `model_spec.constraints` — camelCase field naming (Pitfall 4).
///
/// `aspectRatios` / `defaultResolution` / `resolutions` /
/// `promptCharacterLimit` / `steps` / `widthHeightDivisor`. Do NOT unify this
/// with [`VideoModelConstraints`] via `#[serde(alias)]` — the field sets
/// genuinely differ (image has `steps`/`widthHeightDivisor`; video has
/// `durations`/`model_type`/`audio`).
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageModelConstraints {
    #[serde(default)]
    pub aspect_ratios: Vec<String>,
    #[serde(default)]
    pub default_resolution: String,
    #[serde(default)]
    pub resolutions: Vec<String>,
    #[serde(default)]
    pub prompt_character_limit: u32,
    #[serde(default)]
    pub steps: StepsConstraint,
    #[serde(default)]
    pub width_height_divisor: u32,
}

/// Video `model_spec.constraints` — snake_case field naming (Pitfall 4),
/// matching Rust's default serde convention (no `rename_all` needed).
///
/// `aspect_ratios` / `resolutions` / `durations` / `model_type` / `audio` /
/// `audio_configurable` / `prompt_character_limit`. A populated list constrains
/// its param (matched exactly by [`VeniceClient::resolve_video_duration`] /
/// [`VeniceClient::resolve_video_param`]); an empty/missing list means the model
/// does NOT take that param (e.g. every image-to-video model returns
/// `aspect_ratios: []`, deriving the ratio from the input image), so the param
/// is omitted from the request. The fail-closed guard against an unconstrained
/// request lives one level up in [`video_constraints_for_model`], which rejects
/// any model absent from the catalog.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct VideoModelConstraints {
    #[serde(default)]
    pub aspect_ratios: Vec<String>,
    #[serde(default)]
    pub resolutions: Vec<String>,
    #[serde(default)]
    pub durations: Vec<String>,
    #[serde(default)]
    pub model_type: String,
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub audio_configurable: bool,
    #[serde(default)]
    pub prompt_character_limit: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn generate_image_decodes_base64_to_original_bytes() {
        let server = MockServer::start().await;
        let original_bytes = b"not-really-a-png-but-bytes-are-bytes".to_vec();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&original_bytes);

        Mock::given(method("POST"))
            .and(path("/image/generate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "generate-image-test",
                "images": [b64],
                "request": null,
            })))
            .mount(&server)
            .await;

        let client = VeniceClient::with_base_url(server.uri());
        let bytes = client
            .generate_image(
                "test-key",
                "flux-2-pro",
                "a sunset",
                "webp",
                None,
                None,
                Some(20),
                false,
            )
            .await
            .expect("generate_image should succeed");

        assert_eq!(bytes, original_bytes);
    }

    /// The request body MUST carry `steps` (positive) and the caller's `safe_mode`
    /// — omitting `steps` is what produced under-denoised "blob" images (fix(47)).
    /// The mock only responds when the body matches, so a missing/wrong `steps`
    /// or `safe_mode` makes the request 404 and the call error out.
    #[tokio::test]
    async fn generate_image_sends_steps_and_safe_mode_in_request_body() {
        use wiremock::matchers::body_partial_json;
        let server = MockServer::start().await;
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"bytes-are-bytes");

        Mock::given(method("POST"))
            .and(path("/image/generate"))
            .and(body_partial_json(serde_json::json!({
                "steps": 32,
                "safe_mode": false,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "images": [b64],
            })))
            .mount(&server)
            .await;

        let client = VeniceClient::with_base_url(server.uri());
        client
            .generate_image(
                "test-key",
                "flux-2-pro",
                "a sunset",
                "webp",
                None,
                None,
                Some(32),
                false,
            )
            .await
            .expect("request whose body carries steps=32 + safe_mode=false must match and succeed");
    }

    /// `steps: None`/`Some(0)` omits the field entirely (Venice server-side
    /// fallback) rather than sending `steps: 0`, which would force a blob.
    #[tokio::test]
    async fn generate_image_omits_steps_when_none() {
        use wiremock::matchers::body_json;
        let server = MockServer::start().await;
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"bytes-are-bytes");

        // body_json is an EXACT match: a request that also carried a "steps" key
        // would not equal this steps-less object, so the mock would not respond.
        Mock::given(method("POST"))
            .and(path("/image/generate"))
            .and(body_json(serde_json::json!({
                "model": "flux-2-pro",
                "prompt": "a sunset",
                "format": "webp",
                "return_binary": false,
                "variants": 1,
                "safe_mode": true,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "images": [b64],
            })))
            .mount(&server)
            .await;

        let client = VeniceClient::with_base_url(server.uri());
        client
            .generate_image(
                "test-key",
                "flux-2-pro",
                "a sunset",
                "webp",
                None,
                None,
                None,
                true,
            )
            .await
            .expect("steps=None must omit the field (exact-body mock) and succeed");
    }

    #[tokio::test]
    async fn generate_image_non_2xx_is_terminal_error() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/image/generate"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "error": "content violation",
            })))
            .mount(&server)
            .await;

        let client = VeniceClient::with_base_url(server.uri());
        let result = client
            .generate_image(
                "test-key",
                "flux-2-pro",
                "bad prompt",
                "webp",
                None,
                None,
                Some(20),
                false,
            )
            .await;

        assert!(result.is_err(), "a 422 must not resolve to Ok");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("422"),
            "error should surface the status code, got: {err}"
        );
    }

    #[test]
    fn debug_output_does_not_contain_api_key() {
        let client = VeniceClient::with_base_url("https://api.venice.ai/api/v1");
        let debug_str = format!("{:?}", client);
        assert!(
            !debug_str.contains("super-secret-venice-key"),
            "Debug output must never carry a key value (there is none stored to leak)"
        );
    }

    // -------------------------------------------------------------
    // Task 2: queue_video / retrieve_video
    // -------------------------------------------------------------

    #[tokio::test]
    async fn queue_video_returns_model_and_queue_id() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/video/queue"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "wan-2-7-text-to-video",
                "queue_id": "queue-abc-123",
            })))
            .mount(&server)
            .await;

        let client = VeniceClient::with_base_url(server.uri());
        let job = client
            .queue_video(
                "test-key",
                serde_json::json!({"model": "wan-2-7-text-to-video", "prompt": "a dance"}),
            )
            .await
            .expect("queue_video should succeed");

        assert_eq!(job.model, "wan-2-7-text-to-video");
        assert_eq!(job.queue_id, "queue-abc-123");
        assert_eq!(job.download_url, None);
    }

    #[tokio::test]
    async fn retrieve_video_processing_then_completed_binary() {
        let server = MockServer::start().await;
        let video_bytes = b"fake-mp4-bytes".to_vec();

        // First poll: PROCESSING (json). Second poll: COMPLETED (video/mp4 binary).
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "PROCESSING",
                "average_execution_time": 145000,
                "execution_duration": 53200,
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(video_bytes.clone(), "video/mp4"))
            .mount(&server)
            .await;

        let client = VeniceClient::with_base_url(server.uri());
        let job = QueuedVideoJob {
            model: "wan-2-7-text-to-video".to_string(),
            queue_id: "queue-abc-123".to_string(),
            download_url: None,
        };
        let bytes = client
            .retrieve_video("test-key", &job, Duration::from_secs(5), None)
            .await
            .expect("retrieve_video should succeed");

        assert_eq!(bytes, video_bytes);
    }

    #[tokio::test]
    async fn retrieve_video_completed_json_fetches_download_url() {
        let server = MockServer::start().await;
        let video_bytes = b"private-model-video-bytes".to_vec();

        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "COMPLETED",
                "download_url": format!("{}/downloads/queue-abc-123.mp4", server.uri()),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/downloads/queue-abc-123.mp4"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(video_bytes.clone(), "video/mp4"))
            .mount(&server)
            .await;

        let client = VeniceClient::with_base_url(server.uri());
        let job = QueuedVideoJob {
            model: "wan-2-7-image-to-video".to_string(),
            queue_id: "queue-abc-123".to_string(),
            download_url: None,
        };
        let bytes = client
            .retrieve_video("test-key", &job, Duration::from_secs(5), None)
            .await
            .expect("retrieve_video should follow download_url");

        assert_eq!(bytes, video_bytes);
    }

    #[tokio::test]
    async fn retrieve_video_echoes_model_and_queue_id_every_call() {
        let server = MockServer::start().await;

        // Require BOTH model and queue_id to be present in every retrieve body
        // (Pitfall 2) — a body-matcher mock only responds when both fields
        // match; if the client dropped `model` on a later iteration, this mock
        // would never match and the request would 404 (no fallback mounted).
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "model": "wan-2-7-video-to-video",
                "queue_id": "queue-echo-test",
                "delete_media_on_completion": false,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "PROCESSING",
            })))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "model": "wan-2-7-video-to-video",
                "queue_id": "queue-echo-test",
                "delete_media_on_completion": false,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_raw(b"done".to_vec(), "video/mp4"))
            .mount(&server)
            .await;

        let client = VeniceClient::with_base_url(server.uri());
        let job = QueuedVideoJob {
            model: "wan-2-7-video-to-video".to_string(),
            queue_id: "queue-echo-test".to_string(),
            download_url: None,
        };
        let bytes = client
            .retrieve_video("test-key", &job, Duration::from_secs(5), None)
            .await
            .expect("retrieve_video should succeed when model+queue_id echoed correctly");

        assert_eq!(bytes, b"done".to_vec());
    }

    #[tokio::test]
    async fn retrieve_video_processing_forever_times_out() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "PROCESSING",
            })))
            .mount(&server)
            .await;

        let client = VeniceClient::with_base_url(server.uri());
        let job = QueuedVideoJob {
            model: "wan-2-7-text-to-video".to_string(),
            queue_id: "queue-never-done".to_string(),
            download_url: None,
        };
        let result = client
            .retrieve_video("test-key", &job, Duration::from_millis(50), None)
            .await;

        assert!(result.is_err(), "a job that never completes must time out");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("timed out"),
            "timeout error should say so, got: {err}"
        );
    }

    #[tokio::test]
    async fn retrieve_video_mid_poll_non_2xx_is_terminal_not_retried() {
        let server = MockServer::start().await;

        // First call: PROCESSING. Second call: 422 content-violation — terminal.
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "PROCESSING",
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "error": "content policy violation",
            })))
            .mount(&server)
            .await;

        let client = VeniceClient::with_base_url(server.uri());
        let job = QueuedVideoJob {
            model: "wan-2-7-text-to-video".to_string(),
            queue_id: "queue-fails-midway".to_string(),
            download_url: None,
        };
        let result = client
            .retrieve_video("test-key", &job, Duration::from_secs(5), None)
            .await;

        assert!(result.is_err(), "a mid-poll 422 must be a terminal error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("422"),
            "error should surface the status code, got: {err}"
        );
        assert!(
            !err.to_uppercase().contains("FAILED"),
            "there is no FAILED status in Venice's schema — must not appear in the error"
        );
    }

    #[tokio::test]
    async fn retrieve_video_on_tick_fires_once_per_iteration() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "PROCESSING",
            })))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(b"done".to_vec(), "video/mp4"))
            .mount(&server)
            .await;

        let counter = AtomicUsize::new(0);
        let on_tick = || {
            counter.fetch_add(1, Ordering::SeqCst);
        };

        let client = VeniceClient::with_base_url(server.uri());
        let job = QueuedVideoJob {
            model: "wan-2-7-text-to-video".to_string(),
            queue_id: "queue-tick-test".to_string(),
            download_url: None,
        };
        let bytes = client
            .retrieve_video(
                "test-key",
                &job,
                Duration::from_secs(5),
                Some(&on_tick as &(dyn Fn() + Send + Sync)),
            )
            .await
            .expect("retrieve_video should succeed");

        assert_eq!(bytes, b"done".to_vec());
        assert_eq!(
            counter.load(Ordering::SeqCst),
            3,
            "on_tick must fire once per poll iteration (2 PROCESSING + 1 completing call)"
        );
    }

    // -------------------------------------------------------------
    // Task 3: model_spec constraints + D-13 validator
    // -------------------------------------------------------------

    #[test]
    fn image_model_constraints_deserializes_camel_case() {
        let json = serde_json::json!({
            "aspectRatios": ["1:1", "16:9", "9:16"],
            "defaultResolution": "1K",
            "resolutions": ["1K", "2K", "4K"],
            "promptCharacterLimit": 2048,
            "steps": { "default": 25, "max": 50 },
            "widthHeightDivisor": 8,
        });
        let constraints: ImageModelConstraints = serde_json::from_value(json).unwrap();

        assert_eq!(constraints.aspect_ratios, vec!["1:1", "16:9", "9:16"]);
        assert_eq!(constraints.default_resolution, "1K");
        assert_eq!(constraints.resolutions, vec!["1K", "2K", "4K"]);
        assert_eq!(constraints.prompt_character_limit, 2048);
        assert_eq!(constraints.steps.default, 25);
        assert_eq!(constraints.steps.max, 50);
        assert_eq!(constraints.width_height_divisor, 8);
    }

    #[test]
    fn video_model_constraints_deserializes_snake_case() {
        let json = serde_json::json!({
            "aspect_ratios": ["16:9", "9:16"],
            "resolutions": ["1080p", "720p"],
            "durations": ["5s", "10s", "15s"],
            "model_type": "text-to-video",
            "audio": true,
            "audio_configurable": true,
            "prompt_character_limit": 1000,
        });
        let constraints: VideoModelConstraints = serde_json::from_value(json).unwrap();

        assert_eq!(constraints.aspect_ratios, vec!["16:9", "9:16"]);
        assert_eq!(constraints.resolutions, vec!["1080p", "720p"]);
        assert_eq!(constraints.durations, vec!["5s", "10s", "15s"]);
        assert_eq!(constraints.model_type, "text-to-video");
        assert!(constraints.audio);
        assert!(constraints.audio_configurable);
        assert_eq!(constraints.prompt_character_limit, Some(1000));
    }

    #[test]
    fn camel_case_video_field_never_populates_snake_case_struct() {
        // A payload shaped for VideoModelConstraints (snake_case) parsed as
        // ImageModelConstraints (camelCase) must NOT populate aspect_ratios —
        // the casing mismatch means serde leaves it at the #[serde(default)].
        let json = serde_json::json!({
            "aspect_ratios": ["16:9", "9:16"],
            "durations": ["5s", "10s"],
        });
        let constraints: ImageModelConstraints = serde_json::from_value(json).unwrap();
        assert!(
            constraints.aspect_ratios.is_empty(),
            "snake_case input must not populate the camelCase struct's field"
        );
    }

    #[test]
    fn resolve_video_duration_formats_config_seconds_with_venice_s_suffix() {
        // Venice's catalog lists durations as "5s"/"10s"/"15s" — the config's
        // integer seconds MUST be formatted to match, not sent as a bare "5"
        // (the bug that fail-closed-rejected every Venice video generation).
        let allowed = vec!["5s".to_string(), "10s".to_string(), "15s".to_string()];
        let got = VeniceClient::resolve_video_duration(
            5,
            &allowed,
            "wan-2-7-text-to-video",
            "video_gen.t2v.duration_secs",
        )
        .expect("5 → \"5s\" is in the allowed set");
        assert_eq!(got, Some("5s".to_string()));
    }

    #[test]
    fn resolve_video_duration_rejects_seconds_absent_from_catalog_with_exact_template() {
        let allowed = vec!["5s".to_string(), "10s".to_string(), "15s".to_string()];
        let err = VeniceClient::resolve_video_duration(
            6,
            &allowed,
            "wan-2-7-text-to-video",
            "video_gen.t2v.duration_secs",
        )
        .expect_err("6s is not offered by this model");
        assert_eq!(
            err,
            "duration 6s not supported by wan-2-7-text-to-video; allowed: 5s, 10s, 15s. Fix video_gen.t2v.duration_secs."
        );
    }

    #[test]
    fn resolve_video_duration_honours_auto_only_models() {
        // Video-to-video / upscale models expose exactly ["Auto"]; the config's
        // integer seconds is irrelevant there — honour the model's one option.
        let allowed = vec!["Auto".to_string()];
        let got = VeniceClient::resolve_video_duration(
            5,
            &allowed,
            "wan-2-7-video-to-video",
            "video_gen.v2v.duration_secs",
        )
        .expect("an Auto-only model resolves to Auto");
        assert_eq!(got, Some("Auto".to_string()));
    }

    #[test]
    fn resolve_video_duration_empty_list_omits_the_param() {
        // A catalogued model with no duration constraint ⇒ omit `duration`
        // (None), never a blank/rejected value.
        let got = VeniceClient::resolve_video_duration(
            5,
            &[],
            "some-model",
            "video_gen.t2v.duration_secs",
        )
        .expect("an empty list is not an error");
        assert_eq!(got, None);
    }

    #[test]
    fn resolve_video_param_empty_aspect_ratios_omits_not_rejects() {
        // Every Venice image-to-video model returns aspect_ratios: [] because it
        // derives the ratio from the input image. Empty ⇒ omit (None), NOT the
        // old fail-closed rejection that blocked i2v entirely.
        let got = VeniceClient::resolve_video_param(
            "aspect_ratio",
            "16:9",
            &[],
            "wan-2-7-image-to-video",
            "video_gen.i2v.aspect_ratio",
        )
        .expect("empty aspect_ratios omits the param");
        assert_eq!(got, None);
    }

    #[test]
    fn resolve_video_param_matches_populated_list_and_rejects_outsiders() {
        let allowed = vec!["720p".to_string(), "1080p".to_string()];
        assert_eq!(
            VeniceClient::resolve_video_param(
                "resolution",
                "720p",
                &allowed,
                "wan-2-7-text-to-video",
                "video_gen.t2v.resolution",
            )
            .unwrap(),
            Some("720p".to_string())
        );
        let err = VeniceClient::resolve_video_param(
            "resolution",
            "480p",
            &allowed,
            "wan-2-7-text-to-video",
            "video_gen.t2v.resolution",
        )
        .expect_err("480p is not allowed");
        assert_eq!(
            err,
            "resolution 480p not supported by wan-2-7-text-to-video; allowed: 720p, 1080p. Fix video_gen.t2v.resolution."
        );
    }

    #[test]
    fn video_constraints_for_model_errors_on_uncatalogued_model() {
        // Fail-closed guard: an unknown model is a hard error naming the config
        // key — it can never fall through to the "empty ⇒ omit" resolvers as an
        // unconstrained request.
        let entries = vec![ModelEntry {
            id: "wan-2-7-text-to-video".to_string(),
            constraints: ModelConstraints::Video(VideoModelConstraints::default()),
        }];
        let err = video_constraints_for_model(entries, "venice/text-to-video", "video_gen.t2v")
            .expect_err("an uncatalogued model must be rejected");
        assert!(
            err.to_string()
                .contains("Venice has no video model 'venice/text-to-video'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn video_constraints_for_model_returns_video_constraints_for_a_match() {
        let entries = vec![ModelEntry {
            id: "wan-2-7-text-to-video".to_string(),
            constraints: ModelConstraints::Video(VideoModelConstraints {
                durations: vec!["5s".to_string()],
                ..Default::default()
            }),
        }];
        let c = video_constraints_for_model(entries, "wan-2-7-text-to-video", "video_gen.t2v")
            .expect("a catalogued video model resolves");
        assert_eq!(c.durations, vec!["5s"]);
    }
}

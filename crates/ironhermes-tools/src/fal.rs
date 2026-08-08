//! Phase 01 — fal.ai queue REST client (`FalClient`).
//!
//! A lazy-key, per-call `reqwest` client for the fal.ai queue API
//! (`https://queue.fal.run`). The `FAL_KEY` bearer secret is resolved by the
//! caller (`image_gen`) and passed in per call — `FalClient` never stores it and
//! never logs the `Authorization` header (SAFE-02). The queue base URL is a
//! struct field (default `https://queue.fal.run`) so tests can inject a
//! `wiremock` `server.uri()` (RESEARCH Open Q #3 / 01-SKELETON).
//!
//! State machine (GEN-01): `submit` → `poll` (IN_QUEUE / IN_PROGRESS → COMPLETED)
//! → `fetch` → `download_to_cache` (SSRF-validated streaming download to
//! `~/.ironhermes/cache/images/`, D-01). The full error / timeout / 422 / SSRF
//! rejection test matrix is Plan 02; this module covers the happy path plus the
//! pure helpers (content-type→ext mapping, cache path) under unit test.

use futures::StreamExt;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Default fal queue base URL (overridable for tests).
const DEFAULT_BASE_URL: &str = "https://queue.fal.run";

/// Photo size cap (DLV-04). Mirrors the gateway's 20 MB Telegram photo cap
/// (`SIZE_CAP_20_MB` in `ironhermes-gateway::telegram`), but is owned here so
/// `ironhermes-tools` takes no dependency on `ironhermes-gateway` (circular
/// crate edge). A downloaded result at or under this size is delivered as a
/// native photo; anything larger falls back to a document/text-link.
const PHOTO_SIZE_CAP: u64 = 20 * 1024 * 1024;

/// Maximum number of submit attempts when fal reports a retryable error.
const MAX_SUBMIT_ATTEMPTS: u32 = 4;

/// Outcome of a CDN download (DLV-04).
///
/// `download_to_cache` returns `Photo` when the written file is within the
/// photo cap and `OversizedDocument` when it exceeds the cap. The tool layer
/// (`ImageGenTool::execute`) matches on this to emit either a native photo
/// `<MEDIA:>` tag or a document/text-link fallback — the file is written to the
/// cache in both cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadOutcome {
    /// Within the photo cap — deliver as a native photo attachment.
    Photo(PathBuf),
    /// Over the photo cap — deliver via the document/text-link fallback.
    OversizedDocument(PathBuf),
    /// Within the video cap (D-07, 50 MB) — deliver as a native video attachment.
    Video(PathBuf),
}

/// fal 422 error envelope: `{ "detail": [ { "type": ..., "msg": ... } ] }`.
///
/// fal explicitly warns that client code should branch on `type`, never the
/// human-readable `msg` (which may change). See RESEARCH "Deprecated/outdated".
#[derive(Debug, serde::Deserialize)]
pub struct FalErrorEnvelope {
    #[serde(default)]
    pub detail: Vec<FalErrorItem>,
}

/// A single fal error detail entry.
#[derive(Debug, serde::Deserialize)]
pub struct FalErrorItem {
    #[serde(rename = "type", default)]
    pub err_type: String,
    #[serde(default)]
    pub msg: String,
}

/// The fal error `type` that marks a content-policy rejection (SAFE-04).
const CONTENT_POLICY_VIOLATION: &str = "content_policy_violation";

/// fal queue submit response (subset; RESEARCH Code Examples).
#[derive(Debug, serde::Deserialize)]
pub struct SubmitResponse {
    pub request_id: String,
    #[serde(default)]
    pub status_url: String,
    #[serde(default)]
    pub response_url: String,
    #[serde(default)]
    pub cancel_url: String,
}

/// fal queue status response (subset).
#[derive(Debug, serde::Deserialize)]
pub struct StatusResponse {
    pub status: String,
    #[serde(default)]
    pub queue_position: Option<u32>,
}

/// fal queue result envelope for image models.
#[derive(Debug, serde::Deserialize)]
pub struct ImageResult {
    pub images: Vec<FalImage>,
    #[serde(default)]
    pub seed: Option<i64>,
}

/// A single generated image entry in a fal result.
#[derive(Debug, serde::Deserialize)]
pub struct FalImage {
    pub url: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub content_type: String,
}

/// Video size cap (D-07). 50 MB — Telegram sendVideo ceiling.
/// Deliberately larger than PHOTO_SIZE_CAP (20 MB). Do NOT copy PHOTO_SIZE_CAP here.
pub const VIDEO_SIZE_CAP: u64 = 50 * 1024 * 1024;

/// fal queue result envelope for video models.
///
/// Video models return `{ "video": { "url": "...", ... } }` — NOT `{ "images": [...] }`.
/// A separate struct is required to avoid deserializing video results as `ImageResult`.
#[derive(Debug, serde::Deserialize)]
pub struct VideoResult {
    pub video: FalVideo,
    #[serde(default)]
    pub seed: Option<i64>,
}

/// A single generated video entry in a fal video result.
#[derive(Debug, serde::Deserialize)]
pub struct FalVideo {
    pub url: String,
    #[serde(default)]
    pub content_type: String, // "video/mp4"
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub fps: Option<f32>,
    /// Clip duration in seconds (D-09 tracing: clip_duration_secs).
    #[serde(default)]
    pub duration: Option<f32>,
    #[serde(default)]
    pub num_frames: Option<u32>,
}

/// Lazy-key fal queue client. Holds only the queue base URL — never the key.
///
/// `Debug` is derived safely: the struct carries no secret (the `FAL_KEY` is
/// passed per call and never stored), so there is nothing to redact (SAFE-02).
#[derive(Debug, Clone)]
pub struct FalClient {
    base_url: String,
    /// When true, the CDN-download SSRF gate is skipped. This is ONLY ever set
    /// by the test-only `with_base_url_allow_loopback` constructor so a
    /// `wiremock` server bound to loopback can serve a fake CDN response. The
    /// production constructors (`new`, `with_base_url`) always leave this
    /// `false`, so the SSRF gate is fully active outside tests (SAFE-03). The
    /// gate itself is proven by `test_ssrf_result_url_rejected`.
    allow_loopback_cdn: bool,
}

impl Default for FalClient {
    fn default() -> Self {
        Self::new()
    }
}

impl FalClient {
    /// Construct a client targeting the production fal queue base URL.
    pub fn new() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            allow_loopback_cdn: false,
        }
    }

    /// Construct a client with an injected base URL (test-only entry point).
    ///
    /// The SSRF gate stays active — use `with_base_url_allow_loopback` only when
    /// a loopback-bound mock CDN must be downloaded from.
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            allow_loopback_cdn: false,
        }
    }

    /// Test-only constructor that injects a base URL AND permits downloading
    /// from a loopback CDN (so a `wiremock` server can stand in for the fal
    /// CDN). NEVER used in production code — the SSRF gate is otherwise always
    /// active. `#[doc(hidden)]` to keep it out of the public surface.
    #[doc(hidden)]
    pub fn with_base_url_allow_loopback(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            allow_loopback_cdn: true,
        }
    }

    /// The configured queue base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Whether this client was constructed with loopback-CDN bypass enabled.
    ///
    /// `true` only in test code (`with_base_url_allow_loopback`). Used by
    /// `VideoAnimateTool::resolve_image_url` to skip the SSRF gate on the
    /// `image_url` input when running against a wiremock server on loopback.
    pub fn allow_loopback(&self) -> bool {
        self.allow_loopback_cdn
    }

    /// Validate a queue URL handed back by fal before following it with the
    /// bearer key. Defense-in-depth (SAFE-03 sibling): an authenticated
    /// `poll`/`fetch`/cancel must never follow a `status_url` / `response_url` /
    /// `cancel_url` that points off the configured queue origin — a poisoned
    /// submit response could otherwise redirect the keyed request to an internal
    /// host. Requires the URL to be non-empty and to start with `self.base_url`.
    fn same_origin_url<'u>(&self, url: &'u str, what: &str) -> anyhow::Result<&'u str> {
        if url.is_empty() {
            anyhow::bail!("fal submit response is missing the {what} URL");
        }
        if !url.starts_with(&self.base_url) {
            anyhow::bail!(
                "fal {what} URL is not under the queue base {} — refusing to follow it",
                self.base_url
            );
        }
        Ok(url)
    }

    /// Submit a generation job: `POST {base_url}/{model}`.
    ///
    /// Body is `{ "prompt": <prompt>, "num_inference_steps": 4 }` (flux/schnell
    /// default). On 2xx, deserializes a [`SubmitResponse`]. Non-2xx surfaces the
    /// full fal error body. The `Authorization: Key {fal_key}` header is set
    /// directly and never logged (SAFE-02).
    pub async fn submit(
        &self,
        fal_key: &str,
        model: &str,
        prompt: &str,
    ) -> anyhow::Result<SubmitResponse> {
        let client = reqwest::Client::new();
        let url = format!("{}/{}", self.base_url, model);
        let mut attempt: u32 = 0;
        let mut backoff = Duration::from_millis(250);
        loop {
            attempt += 1;
            let response = client
                .post(&url)
                .header("Authorization", format!("Key {fal_key}"))
                .json(&serde_json::json!({
                    "prompt": prompt,
                    "num_inference_steps": 4
                }))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("fal submit request failed: {}", e))?;
            let status = response.status();
            if status.is_success() {
                return response
                    .json::<SubmitResponse>()
                    .await
                    .map_err(|e| anyhow::anyhow!("fal submit response decode failed: {}", e));
            }

            // Non-2xx: read the retry signal before consuming the body.
            let retryable = is_retryable(&response);
            let body = response.text().await.unwrap_or_default();

            // SAFE-04: a 422 content-policy violation is a terminal, non-retried
            // rejection. Branch on `detail[].type`, NEVER the `msg` string.
            if let Some(item) = content_policy_item(&body) {
                let detail = if item.msg.is_empty() {
                    "content policy violation".to_string()
                } else {
                    item.msg
                };
                return Err(anyhow::anyhow!(
                    "image prompt rejected by fal content policy ({CONTENT_POLICY_VIOLATION}): {detail}"
                ));
            }

            // Retry only when fal says the error is retryable AND we have budget.
            if retryable && attempt < MAX_SUBMIT_ATTEMPTS {
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(5));
                continue;
            }

            return Err(anyhow::anyhow!("fal submit returned {}: {}", status, body));
        }
    }

    /// Poll the job status until COMPLETED or the timeout budget elapses.
    ///
    /// Polls the `status_url` that fal returned in the [`SubmitResponse`] — NOT a
    /// URL reconstructed from the model id. fal collapses sub-models for the queue
    /// status/result/cancel routes (e.g. a submit to `fal-ai/flux/schnell` yields
    /// status/result URLs under the parent app `fal-ai/flux`), so reconstructing
    /// `{base}/{model}/requests/{id}/status` hits a route that rejects GET with a
    /// `405 Method Not Allowed`. Always follow the URLs fal hands back.
    ///
    /// Loops on IN_QUEUE / IN_PROGRESS with capped exponential backoff
    /// (start 500ms, cap 5s) within `timeout`. Returns `Ok(())` on COMPLETED. On
    /// timeout, best-effort cancels via the returned `cancel_url` (GEN-02).
    pub async fn poll(
        &self,
        fal_key: &str,
        submit: &SubmitResponse,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let client = reqwest::Client::new();
        let url = self.same_origin_url(&submit.status_url, "status")?;
        let deadline = tokio::time::Instant::now() + timeout;
        let mut delay = Duration::from_millis(500);
        loop {
            if tokio::time::Instant::now() >= deadline {
                // GEN-02: cancel the in-flight job so fal stops working/charging.
                // Best-effort — a cancel failure (incl. a missing/foreign cancel
                // URL) must not mask the timeout error.
                if let Ok(cancel_url) = self.same_origin_url(&submit.cancel_url, "cancel") {
                    let _ = client
                        .put(cancel_url)
                        .header("Authorization", format!("Key {fal_key}"))
                        .send()
                        .await;
                }
                anyhow::bail!("image generation timed out after {:?}", timeout);
            }
            let response = client
                .get(url)
                .header("Authorization", format!("Key {fal_key}"))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("fal status request failed: {}", e))?;
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(anyhow::anyhow!("fal status returned {}: {}", status, body));
            }
            let st: StatusResponse = response
                .json()
                .await
                .map_err(|e| anyhow::anyhow!("fal status decode failed: {}", e))?;
            match st.status.as_str() {
                "COMPLETED" => return Ok(()),
                "IN_QUEUE" | "IN_PROGRESS" => {
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(5));
                }
                other => anyhow::bail!("unexpected fal status: {other}"),
            }
        }
    }

    /// Fetch the completed result by following the `response_url` fal returned in
    /// the [`SubmitResponse`] (NOT a URL reconstructed from the model id — see
    /// [`poll`](Self::poll) for why reconstruction 405s on sub-models).
    pub async fn fetch(
        &self,
        fal_key: &str,
        submit: &SubmitResponse,
    ) -> anyhow::Result<ImageResult> {
        let client = reqwest::Client::new();
        let url = self.same_origin_url(&submit.response_url, "response")?;
        let response = client
            .get(url)
            .header("Authorization", format!("Key {fal_key}"))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("fal result request failed: {}", e))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("fal result returned {}: {}", status, body));
        }
        response
            .json::<ImageResult>()
            .await
            .map_err(|e| anyhow::anyhow!("fal result decode failed: {}", e))
    }

    /// SSRF-validate the CDN URL, then stream-download it to the image cache.
    ///
    /// Validates with `is_safe_url` off the async worker via `spawn_blocking`
    /// (SAFE-03) and rejects on `false`, creates
    /// `get_hermes_home()/cache/images` (D-01), streams the body to a
    /// `<UTC %Y%m%dT%H%M%S>-<uuid v4>.<ext>` file (ext inferred from
    /// `content_type`, D-02), and returns the absolute [`PathBuf`].
    pub async fn download_to_cache(
        &self,
        image_url: &str,
        content_type: &str,
    ) -> anyhow::Result<DownloadOutcome> {
        self.download_to_cache_with_cap(image_url, content_type, PHOTO_SIZE_CAP)
            .await
    }

    /// Like [`download_to_cache`](Self::download_to_cache) but with an explicit
    /// photo-cap threshold (test seam, DLV-04).
    ///
    /// SSRF-validates, streams the body to the cache, then classifies the
    /// written file against `photo_cap`: at or under the cap → [`DownloadOutcome::Photo`],
    /// over the cap → [`DownloadOutcome::OversizedDocument`]. The file is written
    /// in both cases (the fallback references the same on-disk path).
    pub async fn download_to_cache_with_cap(
        &self,
        image_url: &str,
        content_type: &str,
        photo_cap: u64,
    ) -> anyhow::Result<DownloadOutcome> {
        // SAFE-03: validate BEFORE the GET, off the async runtime thread (sync DNS).
        // The `allow_loopback_cdn` bypass is test-only (see the field doc); in
        // production it is always false, so the gate always runs.
        if !self.allow_loopback_cdn {
            let u = image_url.to_string();
            let safe =
                tokio::task::spawn_blocking(move || ironhermes_core::is_safe_url(&u)).await?;
            if !safe {
                anyhow::bail!("fal result URL rejected by SSRF check");
            }
        }

        let abs_path = cache_image_path(content_type);
        if let Some(parent) = abs_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let client = reqwest::Client::new();
        let response = client
            .get(image_url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("fal CDN download request failed: {}", e))?
            .error_for_status()
            .map_err(|e| anyhow::anyhow!("fal CDN download failed: {}", e))?;
        let mut stream = response.bytes_stream();

        let mut written: u64 = 0;
        let mut file = tokio::fs::File::create(&abs_path).await?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow::anyhow!("fal CDN stream error: {}", e))?;
            written += chunk.len() as u64;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;

        // DLV-04: size-gate the written file against the photo cap.
        if written > photo_cap {
            Ok(DownloadOutcome::OversizedDocument(abs_path))
        } else {
            Ok(DownloadOutcome::Photo(abs_path))
        }
    }

    /// Submit a generation job with an arbitrary JSON body: `POST {base_url}/{model}`.
    ///
    /// Mirrors [`submit`](Self::submit) exactly (retry/backoff, Authorization, content-policy,
    /// `is_retryable`, `MAX_SUBMIT_ATTEMPTS`) but accepts an arbitrary `serde_json::Value` body
    /// instead of the hardcoded image body. This is the entry point for video models (T2V / I2V)
    /// which use different request fields. The existing `submit()` is left byte-stable so
    /// `image_gen` continues to use it without change (Pitfall 1).
    ///
    /// The `Authorization: Key {fal_key}` header is never logged (SAFE-02). A
    /// `content_policy_violation` 422 is a terminal, non-retried rejection (SAFE-04).
    pub async fn submit_with_body(
        &self,
        fal_key: &str,
        model: &str,
        body: serde_json::Value,
    ) -> anyhow::Result<SubmitResponse> {
        let client = reqwest::Client::new();
        let url = format!("{}/{}", self.base_url, model);
        let mut attempt: u32 = 0;
        let mut backoff = Duration::from_millis(250);
        loop {
            attempt += 1;
            let response = client
                .post(&url)
                .header("Authorization", format!("Key {fal_key}"))
                .json(&body)
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("fal submit request failed: {}", e))?;
            let status = response.status();
            if status.is_success() {
                return response
                    .json::<SubmitResponse>()
                    .await
                    .map_err(|e| anyhow::anyhow!("fal submit response decode failed: {}", e));
            }

            // Non-2xx: read the retry signal before consuming the body.
            let retryable = is_retryable(&response);
            let body_text = response.text().await.unwrap_or_default();

            // SAFE-04: a 422 content-policy violation is a terminal, non-retried
            // rejection. Branch on `detail[].type`, NEVER the `msg` string.
            if let Some(item) = content_policy_item(&body_text) {
                let detail = if item.msg.is_empty() {
                    "content policy violation".to_string()
                } else {
                    item.msg
                };
                return Err(anyhow::anyhow!(
                    "video prompt rejected by fal content policy ({CONTENT_POLICY_VIOLATION}): {detail}"
                ));
            }

            // Retry only when fal says the error is retryable AND we have budget.
            if retryable && attempt < MAX_SUBMIT_ATTEMPTS {
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(5));
                continue;
            }

            return Err(anyhow::anyhow!(
                "fal submit returned {}: {}",
                status,
                body_text
            ));
        }
    }

    /// Fetch the completed video result by following the `response_url` fal returned in
    /// the [`SubmitResponse`]. Mirrors [`fetch`](Self::fetch) but deserializes [`VideoResult`]
    /// (`{ "video": { "url": ... } }`) instead of `ImageResult` (`{ "images": [...] }`) —
    /// Pitfall 2: these response shapes are incompatible.
    pub async fn fetch_video(
        &self,
        fal_key: &str,
        submit: &SubmitResponse,
    ) -> anyhow::Result<VideoResult> {
        let client = reqwest::Client::new();
        let url = self.same_origin_url(&submit.response_url, "response")?;
        let response = client
            .get(url)
            .header("Authorization", format!("Key {fal_key}"))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("fal result request failed: {}", e))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("fal result returned {}: {}", status, body));
        }
        response
            .json::<VideoResult>()
            .await
            .map_err(|e| anyhow::anyhow!("fal video result decode failed: {}", e))
    }

    /// SSRF-validate the CDN URL, then stream-download it to the video cache.
    ///
    /// Delegates to [`download_video_to_cache_with_cap`](Self::download_video_to_cache_with_cap)
    /// using [`VIDEO_SIZE_CAP`] (50 MB, D-07). The cap is intentionally larger than
    /// `PHOTO_SIZE_CAP` (20 MB) — do not reuse the photo cap for video (Pitfall 4).
    pub async fn download_video_to_cache(
        &self,
        video_url: &str,
        content_type: &str,
    ) -> anyhow::Result<DownloadOutcome> {
        self.download_video_to_cache_with_cap(video_url, content_type, VIDEO_SIZE_CAP)
            .await
    }

    /// Like [`download_video_to_cache`](Self::download_video_to_cache) but with an explicit
    /// video-cap threshold (test seam, D-07).
    ///
    /// SSRF-validates (SAFE-03), streams the body to `cache/videos/` (D-03 — NOT
    /// `cache/images/`, Pitfall 3), then classifies the written file against `video_cap`:
    /// at or under the cap → [`DownloadOutcome::Video`], over the cap →
    /// [`DownloadOutcome::OversizedDocument`]. The file is written in both cases.
    pub async fn download_video_to_cache_with_cap(
        &self,
        video_url: &str,
        content_type: &str,
        video_cap: u64,
    ) -> anyhow::Result<DownloadOutcome> {
        // SAFE-03: validate BEFORE the GET, off the async runtime thread (sync DNS).
        // The `allow_loopback_cdn` bypass is test-only; in production it is always false.
        if !self.allow_loopback_cdn {
            let u = video_url.to_string();
            let safe =
                tokio::task::spawn_blocking(move || ironhermes_core::is_safe_url(&u)).await?;
            if !safe {
                anyhow::bail!("fal video result URL rejected by SSRF check");
            }
        }

        let abs_path = cache_video_path(content_type);
        if let Some(parent) = abs_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let client = reqwest::Client::new();
        let response = client
            .get(video_url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("fal CDN video download request failed: {}", e))?
            .error_for_status()
            .map_err(|e| anyhow::anyhow!("fal CDN video download failed: {}", e))?;
        let mut stream = response.bytes_stream();

        let mut written: u64 = 0;
        let mut file = tokio::fs::File::create(&abs_path).await?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow::anyhow!("fal CDN video stream error: {}", e))?;
            written += chunk.len() as u64;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;

        // D-07: size-gate the written file against the video cap.
        if written > video_cap {
            Ok(DownloadOutcome::OversizedDocument(abs_path))
        } else {
            Ok(DownloadOutcome::Video(abs_path))
        }
    }
}

/// Read the `X-Fal-Retryable` header. Absent / unparsable → non-retryable
/// (fail-closed: never retry an error fal did not mark retryable).
fn is_retryable(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get("X-Fal-Retryable")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Parse a fal error body and return the first `content_policy_violation`
/// detail item, if any. Branches on `detail[].type`, never `msg` (SAFE-04).
fn content_policy_item(body: &str) -> Option<FalErrorItem> {
    serde_json::from_str::<FalErrorEnvelope>(body)
        .ok()?
        .detail
        .into_iter()
        .find(|item| item.err_type == CONTENT_POLICY_VIOLATION)
}

/// Map a fal `content_type` to a file extension (D-02 filename scheme).
///
/// Falls back to `png` for unknown / empty content types — fal image models
/// return `image/png` by default for flux/schnell.
pub(crate) fn ext_for_content_type(content_type: &str) -> &'static str {
    // Trim any `; charset=...` suffix and lowercase for a robust match.
    let main = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match main.as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    }
}

/// Map a fal video `content_type` to a file extension (D-03 filename scheme).
///
/// Falls back to `mp4` for unknown / empty content types — fal video models
/// return `video/mp4` by default.
pub(crate) fn ext_for_video_content_type(content_type: &str) -> &'static str {
    let main = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match main.as_str() {
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        _ => "mp4",
    }
}

/// Build the absolute cache path for a downloaded video (D-03).
///
/// `get_hermes_home()/cache/videos/<UTC %Y%m%dT%H%M%S>-<uuid v4>.<ext>`.
/// Uses `cache/videos/` (NOT `cache/images/` — Pitfall 3).
/// The path is always absolute and unique; the directory is created by the caller before writing.
pub(crate) fn cache_video_path(content_type: &str) -> PathBuf {
    let dir = ironhermes_core::constants::get_hermes_home()
        .join("cache")
        .join("videos");
    let ext = ext_for_video_content_type(content_type);
    let fname = format!(
        "{}-{}.{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S"),
        uuid::Uuid::new_v4(),
        ext
    );
    dir.join(fname)
}

/// Build the absolute cache path for a downloaded image (D-01 / D-02).
///
/// `get_hermes_home()/cache/images/<UTC %Y%m%dT%H%M%S>-<uuid v4>.<ext>`.
/// The path is always absolute and unique; the directory is created by the
/// caller before writing.
pub(crate) fn cache_image_path(content_type: &str) -> PathBuf {
    let dir = ironhermes_core::constants::get_hermes_home()
        .join("cache")
        .join("images");
    let ext = ext_for_content_type(content_type);
    let fname = format!(
        "{}-{}.{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S"),
        uuid::Uuid::new_v4(),
        ext
    );
    dir.join(fname)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_png_maps_to_png() {
        assert_eq!(ext_for_content_type("image/png"), "png");
    }

    #[test]
    fn content_type_jpeg_maps_to_jpg() {
        assert_eq!(ext_for_content_type("image/jpeg"), "jpg");
    }

    #[test]
    fn content_type_webp_maps_to_webp() {
        assert_eq!(ext_for_content_type("image/webp"), "webp");
    }

    #[test]
    fn content_type_gif_maps_to_gif() {
        assert_eq!(ext_for_content_type("image/gif"), "gif");
    }

    #[test]
    fn content_type_with_charset_suffix_still_maps() {
        assert_eq!(ext_for_content_type("image/jpeg; charset=binary"), "jpg");
    }

    #[test]
    fn content_type_unknown_falls_back_to_png() {
        assert_eq!(ext_for_content_type("application/octet-stream"), "png");
        assert_eq!(ext_for_content_type(""), "png");
    }

    #[test]
    fn cache_path_is_absolute_and_under_hermes_home_images() {
        let expected_dir = ironhermes_core::constants::get_hermes_home()
            .join("cache")
            .join("images");
        let path = cache_image_path("image/png");
        assert!(path.is_absolute(), "cache image path must be absolute");
        assert!(
            path.starts_with(&expected_dir),
            "path {:?} must be under {:?}",
            path,
            expected_dir
        );
        assert_eq!(
            path.extension().and_then(|e| e.to_str()),
            Some("png"),
            "png content-type must yield a .png file"
        );
    }

    #[test]
    fn cache_paths_are_unique() {
        let a = cache_image_path("image/png");
        let b = cache_image_path("image/png");
        assert_ne!(a, b, "uuid v4 must make each cache path unique");
    }

    #[test]
    fn default_base_url_is_fal_queue() {
        assert_eq!(FalClient::new().base_url(), "https://queue.fal.run");
    }

    #[test]
    fn injected_base_url_is_used() {
        let c = FalClient::with_base_url("http://127.0.0.1:1234");
        assert_eq!(c.base_url(), "http://127.0.0.1:1234");
    }
}

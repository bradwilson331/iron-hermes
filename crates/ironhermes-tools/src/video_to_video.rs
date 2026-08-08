//! Phase 47 Plan 07 — `video_to_video` (v2v) LLM tool: the NET-NEW generation
//! mode this phase introduces (D-14, GEN-02).
//!
//! Mirrors `VideoAnimateTool` (`video_gen.rs`) almost line-for-line: the async
//! Venice submit-then-poll lifecycle (or the fal.ai queue pipeline when an
//! operator explicitly configures a `fal-ai/*` v2v model), the
//! [`GenerationGuardrail`] chokepoint enforced BEFORE any provider call, the
//! D-13 fail-closed `model_spec` validation, the 300s timeout + config-gated
//! progress ping, and the unchanged `<MEDIA:>` emit are all reused as-is. The
//! only genuinely new piece is [`resolve_video_url`] — a `resolve_image_url`
//! analog for the v2v input reference (SSRF + path-traversal + size-gate-
//! before-read guard), and the Venice `video_url` request field.
//!
//! A handful of small helpers (`venice_video_error`, the progress-ping
//! ticker, the per-session slot reservation) are duplicated here rather than
//! imported from `video_gen.rs`, because they are private to that module and
//! this plan's `files_modified` scope is `video_to_video.rs` + `lib.rs` only
//! — `video_gen.rs` is intentionally left untouched by this plan.
//!
//! `is_available()`/`prerequisites()` are dynamic (unlike the sibling t2v/i2v
//! tools' static `FAL_KEY`-only gate): v2v has no legacy fal default (D-14 —
//! its D-03 default provider is venice), so the tool advertises BOTH
//! possible keys and gates on whichever one the CONFIGURED effective model's
//! resolved backend actually needs (the "either KEY_A or KEY_B" override the
//! `Tool` trait doc calls out).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _; // CRITICAL: must be in scope for .encode() on STANDARD engine
use ironhermes_core::{Config, SessionKey, ToolSchema};
use serde_json::{Value, json};
use tracing::debug;

use crate::fal::{DownloadOutcome, FalClient};
use crate::gen_backend::{self, GenBackend};
use crate::gen_guardrail::{GenerationGuardrail, ReservationKind};
use crate::image_gen::{AckSink, ArtifactCaptureSink};
use crate::registry::Tool;
use crate::venice::{QueuedVideoJob, VeniceClient, video_constraints_for_model};
use crate::video_gen::VideoSessionCounter;

/// Map a Venice video-path error to the exact UI-SPEC non-retried strings
/// (D-13/UI-SPEC). Identical mapping to `video_gen.rs::venice_video_error`,
/// duplicated locally (see module doc: `video_gen.rs` is out of this plan's
/// file scope). Venice's `/video/retrieve` schema has no `FAILED` status —
/// the ONLY Venice error whose message contains "timed out" is the
/// poll-deadline error `retrieve_video` returns, so that substring is the
/// sole discriminant between the two templates.
fn venice_video_error(e: anyhow::Error, timeout: Duration) -> anyhow::Error {
    let msg = e.to_string();
    if msg.contains("timed out") {
        anyhow::anyhow!("Video generation timed out after {}s.", timeout.as_secs())
    } else {
        anyhow::anyhow!("Video generation failed: {msg}.")
    }
}

/// Config-gated periodic "Still working on your video…" progress ping
/// (UI-SPEC). Identical copy/gating to `video_gen.rs::ProgressPinger`,
/// duplicated locally for the same file-scope reason (see module doc).
struct ProgressPinger {
    ack_sink: Option<Arc<dyn AckSink>>,
    cadence: Duration,
    /// Seeded to "now" at construction (polling start), not `None` — so the
    /// first re-ping only fires once a FULL cadence has elapsed since polling
    /// began, not on the very first `on_tick` call.
    last_ping: Mutex<Option<std::time::Instant>>,
}

impl ProgressPinger {
    fn new(cadence: Duration, ack_sink: Option<Arc<dyn AckSink>>) -> Self {
        Self {
            ack_sink,
            cadence,
            last_ping: Mutex::new(Some(std::time::Instant::now())),
        }
    }

    fn tick(&self) {
        if self.cadence.is_zero() {
            return;
        }
        let Some(sink) = &self.ack_sink else {
            return;
        };
        let now = std::time::Instant::now();
        let mut last = self.last_ping.lock().unwrap_or_else(|e| e.into_inner());
        let should_ping = match *last {
            None => true,
            Some(prev) => now.duration_since(prev) >= self.cadence,
        };
        if should_ping {
            *last = Some(now);
            drop(last);
            sink.ack("Still working on your video…");
        }
    }
}

/// Drive `VeniceClient::retrieve_video` with the config-gated periodic
/// progress ping wired to its `on_tick` hook — identical to
/// `video_gen.rs::retrieve_video_with_progress_ping`, duplicated locally.
async fn retrieve_video_with_progress_ping(
    venice_client: &VeniceClient,
    venice_key: &str,
    job: &QueuedVideoJob,
    timeout: Duration,
    progress_ping_secs: u64,
    ack_sink: Option<Arc<dyn AckSink>>,
) -> anyhow::Result<Vec<u8>> {
    let pinger = ProgressPinger::new(Duration::from_secs(progress_ping_secs), ack_sink);
    let on_tick = move || pinger.tick();
    venice_client
        .retrieve_video(
            venice_key,
            job,
            timeout,
            Some(&on_tick as &(dyn Fn() + Send + Sync)),
        )
        .await
}

/// RAII reservation for one in-flight v2v generation slot, sharing the same
/// [`VideoSessionCounter`] (and hence the same paid-video quota,
/// `video_gen.session_cap`, D-06) as `VideoGenerateTool`/`VideoAnimateTool`.
///
/// Duplicated locally rather than reusing `video_gen::SlotGuard` because that
/// type's fields (and its `inert()` constructor) are private to `video_gen`
/// and cannot be constructed outside it — see module doc for the file-scope
/// rationale. Semantics are identical: the slot is reserved (incremented) the
/// moment the guard is created under the same lock scope as the cap check
/// (closing the TOCTOU/concurrency hole), and released on `Drop` unless
/// [`V2vSlotGuard::commit`] is called.
#[must_use = "the reservation is released when the guard is dropped; hold it for the lifetime of the generation"]
struct V2vSlotGuard {
    /// `None` for the stateless (no-cap) path — nothing to release or commit.
    inner: Option<(SessionKey, VideoSessionCounter)>,
    committed: bool,
}

impl V2vSlotGuard {
    /// Inert guard for stateless tools (no cap enforced).
    fn inert() -> Self {
        Self {
            inner: None,
            committed: true, // nothing reserved; nothing to release on drop
        }
    }

    /// Commit the reservation: the in-flight slot becomes a recorded success
    /// and will NOT be decremented on drop.
    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for V2vSlotGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Some((key, counter)) = &self.inner {
            let mut guard = counter.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(slot) = guard.get_mut(key) {
                *slot = slot.saturating_sub(1);
            }
        }
    }
}

/// Atomically check the per-session video cap and reserve a slot in a single
/// lock scope — identical logic to `video_gen.rs::reserve_video_slot`,
/// duplicated locally (same type, same cap, same behavior; see module doc).
fn reserve_video_slot(
    session_key: &Option<SessionKey>,
    counter: &Option<VideoSessionCounter>,
    cap: u32,
) -> Result<V2vSlotGuard, String> {
    let (Some(key), Some(counter)) = (session_key, counter) else {
        return Ok(V2vSlotGuard::inert());
    };
    let mut map = counter.lock().unwrap_or_else(|e| e.into_inner());
    let used = *map.get(key).unwrap_or(&0);
    if used >= cap {
        return Err(format!(
            "Per-session video generation limit reached ({cap} videos for this chat session). \
             This is a hard limit to prevent runaway generation; no further videos will be \
             generated in this session."
        ));
    }
    *map.entry(key.clone()).or_insert(0) += 1;
    drop(map);
    Ok(V2vSlotGuard {
        inner: Some((key.clone(), counter.clone())),
        committed: false,
    })
}

/// Map a video file extension to its MIME type for the base64 data URI.
/// Video-specific analog of `video_gen.rs::mime_from_extension`.
fn video_mime_from_extension(ext: Option<&str>) -> &'static str {
    match ext {
        Some("mp4") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        // Unknown/missing extension -> application/octet-stream, mirroring
        // the image-tool WR-05 fallback contract (never guess a specific type).
        _ => "application/octet-stream",
    }
}

/// Video-to-video tool backed by Venice.ai's async submit-then-poll `/video`
/// lifecycle (D-14 default provider, D-03) or the fal.ai queue API when an
/// operator explicitly configures a `fal-ai/*` v2v model (D-02).
///
/// `video_url` accepts:
/// - A public HTTPS URL: validated by `is_safe_url` in `spawn_blocking`, then
///   passed unchanged.
/// - A local filesystem path: must canonicalize under
///   `$IRONHERMES_HOME/cache/` (path traversal guard), size-gated via
///   `tokio::fs::metadata` BEFORE `tokio::fs::read` (CR-01 memory-exhaustion
///   guard), then encoded as a `data:<mime>;base64,<b64>` URI.
pub struct VideoToVideoTool {
    config: Arc<Config>,
    /// fal.ai client — resolved backend per-call via `GenBackend::resolve`
    /// (D-02). No default v2v fal model is configured (D-03: v2v defaults to
    /// venice-only `wan-2-7-video-to-video`), but an operator may still
    /// explicitly configure a `fal-ai/*` v2v model.
    client: FalClient,
    /// Venice.ai client (D-01/D-02/D-03). Defaults to production
    /// `VeniceClient::new()`; overridable via
    /// [`VideoToVideoTool::with_venice_client`].
    venice_client: VeniceClient,
    session_key: Option<SessionKey>,
    counter: Option<VideoSessionCounter>,
    ack_sink: Option<Arc<dyn AckSink>>,
    /// Shared spend guardrail (Plan 05, GEN-05). `None` = not enforced
    /// (pre-Plan-08 call sites) — the per-session cap still applies.
    guardrail: Option<Arc<GenerationGuardrail>>,
    /// D-08: `Root` (direct chat, exempt from `per_child_cap`/the descendant
    /// pool) or `Descendant` (delegate/kanban, subject to both tiers).
    /// Defaults to `Root`; wired by Plan 08 at production construction time.
    reservation_kind: ReservationKind,
    /// Plan 08 (D-10): optional artifact-gallery capture sink, wired only for
    /// delegate children.
    capture_sink: Option<Arc<dyn ArtifactCaptureSink>>,
}

impl VideoToVideoTool {
    /// Stateless construction — no cap enforced. Used in unit tests.
    pub fn new(config: Arc<Config>, client: FalClient) -> Self {
        Self {
            config,
            client,
            venice_client: VeniceClient::new(),
            session_key: None,
            counter: None,
            ack_sink: None,
            guardrail: None,
            reservation_kind: ReservationKind::Root,
            capture_sink: None,
        }
    }

    /// Per-session construction (production path). The cap is scoped to
    /// `session_key` and shares the SAME `VideoSessionCounter` (and hence the
    /// same `video_gen.session_cap` quota) as `VideoGenerateTool`/
    /// `VideoAnimateTool` when the caller passes in the same counter Arc.
    pub fn new_with_session(
        session_key: SessionKey,
        config: Arc<Config>,
        client: FalClient,
        counter: VideoSessionCounter,
        ack_sink: Option<Arc<dyn AckSink>>,
    ) -> Self {
        Self {
            config,
            client,
            venice_client: VeniceClient::new(),
            session_key: Some(session_key),
            counter: Some(counter),
            ack_sink,
            guardrail: None,
            reservation_kind: ReservationKind::Root,
            capture_sink: None,
        }
    }

    /// Inject the shared [`GenerationGuardrail`] (Plan 05, GEN-05) and its
    /// [`ReservationKind`]. Builder-style so `new`/`new_with_session` never
    /// need a new required parameter (mirrors the sibling video tools).
    #[must_use]
    pub fn with_guardrail(
        mut self,
        guardrail: Arc<GenerationGuardrail>,
        kind: ReservationKind,
    ) -> Self {
        self.guardrail = Some(guardrail);
        self.reservation_kind = kind;
        self
    }

    /// Inject the artifact-gallery capture sink (Plan 08, D-10) — wired only
    /// for delegate children via `build_child_registry`.
    #[must_use]
    pub fn with_capture_sink(mut self, sink: Arc<dyn ArtifactCaptureSink>) -> Self {
        self.capture_sink = Some(sink);
        self
    }

    /// Override the Venice client (test-only entry point).
    #[must_use]
    pub fn with_venice_client(mut self, client: VeniceClient) -> Self {
        self.venice_client = client;
        self
    }

    /// Cap check + atomic reservation (D-06 / WR-01) — shares the same
    /// counter/cap semantics as the sibling video tools' `try_reserve_slot`.
    fn try_reserve_slot(&self) -> Result<V2vSlotGuard, String> {
        reserve_video_slot(
            &self.session_key,
            &self.counter,
            self.config.video_gen.session_cap,
        )
    }

    fn emit_interim_ack(&self) {
        if let Some(sink) = &self.ack_sink {
            // UI-SPEC: reuse the exact shipped video ack string verbatim — a
            // video is a video; v2v gets no different ack copy than t2v/i2v.
            sink.ack("Generating your video… this may take a few minutes.");
        }
    }

    /// Resolve the `video_url` argument into a provider-safe string:
    ///
    /// - `data:` URIs are rejected — the model must pass a cache path or a
    ///   public URL, never inline video data (mirrors the `resolve_image_url`
    ///   `data:` reject in `video_gen.rs`).
    /// - `https://`/`http://` URLs: validated with `is_safe_url` in
    ///   `spawn_blocking` (SSRF guard) unless `skip_ssrf` is `true`
    ///   (test-only, via `FalClient::allow_loopback()`).
    /// - Everything else is treated as a local path: `canonicalize()` →
    ///   assert prefix under `$IRONHERMES_HOME/cache/` (path traversal guard)
    ///   → size-gate against `max_inline_bytes` via `tokio::fs::metadata`
    ///   BEFORE `tokio::fs::read` (CR-01 memory-exhaustion DoS guard) → read
    ///   bytes → detect MIME from extension → encode as
    ///   `data:<mime>;base64,<b64>` URI.
    async fn resolve_video_url(
        raw: &str,
        skip_ssrf: bool,
        max_inline_bytes: u64,
    ) -> anyhow::Result<String> {
        if raw.starts_with("data:") {
            anyhow::bail!(
                "video_to_video needs a video file path or a public URL, not inline video data. \
                 Pass the cache path of the video (e.g. a prior video_generate/video_animate \
                 result)."
            );
        }
        if raw.starts_with("https://") || raw.starts_with("http://") {
            if !skip_ssrf {
                // SSRF guard: run is_safe_url off the async runtime thread (sync DNS).
                let owned = raw.to_string();
                let safe =
                    tokio::task::spawn_blocking(move || ironhermes_core::is_safe_url(&owned))
                        .await
                        .map_err(|e| anyhow::anyhow!("SSRF check task panicked: {e}"))?;
                if !safe {
                    anyhow::bail!(
                        "video_url rejected by SSRF check: only publicly accessible URLs are allowed"
                    );
                }
            }
            return Ok(raw.to_string());
        }

        // Local path branch — must be under the cache root.
        let path = std::path::Path::new(raw);
        let canonical = tokio::fs::canonicalize(path)
            .await
            .map_err(|e| anyhow::anyhow!("video_url path could not be resolved: {e}"))?;

        let cache_root = ironhermes_core::constants::get_hermes_home().join("cache");
        if !canonical.starts_with(&cache_root) {
            anyhow::bail!(
                "video_url path is outside the allowed cache root ({}) — path traversal rejected",
                cache_root.display()
            );
        }

        // CR-01: size-gate BEFORE reading the file into memory — the cache
        // root contains LLM-written generated media of unbounded size; an
        // unbounded `read` + base64 (~1.33x) + format! (a third copy) of a
        // huge file is a memory-exhaustion DoS. Stat first and reject
        // over-cap with a clear, non-retried error and NO provider call.
        let meta = tokio::fs::metadata(&canonical)
            .await
            .map_err(|e| anyhow::anyhow!("video_url path could not be stat'd: {e}"))?;
        if meta.len() > max_inline_bytes {
            anyhow::bail!(
                "video file is too large to inline ({} bytes > {} cap)",
                meta.len(),
                max_inline_bytes
            );
        }

        let bytes = tokio::fs::read(&canonical)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read video file: {e}"))?;

        let mime = video_mime_from_extension(canonical.extension().and_then(|e| e.to_str()));
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(format!("data:{mime};base64,{b64}"))
    }
}

#[async_trait]
impl Tool for VideoToVideoTool {
    fn name(&self) -> &str {
        "video_to_video"
    }

    fn toolset(&self) -> &str {
        "web"
    }

    fn description(&self) -> &str {
        "Transform an existing video clip into a new video clip (motion/style transfer) and \
         deliver it to the user as a native attachment. Provide `video_url` (a public HTTPS URL \
         or a local cache path) and an optional `prompt` to guide the transformation."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "video_to_video",
            "Transform an existing video clip into a new video clip and deliver it as a native \
             attachment.",
            json!({
                "type": "object",
                "properties": {
                    "video_url": {
                        "type": "string",
                        "description": "A publicly accessible HTTPS URL or a local cache path to \
                                        the video to transform."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Optional text prompt to guide the transformation (e.g. \
                                        \"turn into a cartoon\", \"add falling snow\")."
                    },
                    "model": {
                        "type": "string",
                        "description": "Optional model id to override the configured default \
                                        (e.g. \"wan-2-7-video-to-video\")."
                    }
                },
                "required": ["video_url"]
            }),
        )
    }

    /// D-12/D-25: advertise BOTH possible provider keys for setup-wizard
    /// discovery — v2v has no legacy fal default (unlike t2v/i2v), so there
    /// is no single hardcoded key to name. `is_available()` below is the
    /// real gate and picks the one the CONFIGURED effective model actually
    /// needs.
    fn prerequisites(&self) -> Vec<crate::registry::Prerequisite> {
        vec![
            crate::registry::Prerequisite {
                kind: "env_var".to_string(),
                name: "VENICE_API_KEY".to_string(),
                description: "Venice.ai API key — required for video_to_video when using a \
                              Venice model (the D-03 default)."
                    .to_string(),
                required: true,
                group: None,
            },
            crate::registry::Prerequisite {
                kind: "env_var".to_string(),
                name: "FAL_KEY".to_string(),
                description: "fal.ai API key — required for video_to_video when configured to \
                              use a fal-ai/* model."
                    .to_string(),
                required: true,
                group: None,
            },
        ]
    }

    /// Overridden (not the default `prerequisites()`-walking behavior): gate
    /// on whichever key the CONFIGURED `video_gen.v2v` effective model's
    /// resolved backend (D-02) actually needs — never both, never neither.
    fn is_available(&self) -> bool {
        match gen_backend::resolve(
            self.config.video_gen.v2v.provider.as_deref(),
            &self.config.video_gen.v2v.model,
        ) {
            Ok(GenBackend::Fal(_)) => std::env::var("FAL_KEY").is_ok(),
            Ok(GenBackend::Venice(_)) => std::env::var("VENICE_API_KEY").is_ok(),
            Err(_) => false,
        }
    }

    /// D-04 (Phase 41.3): provider-side video generation routinely exceeds
    /// the 60s trait default.
    fn timeout_secs(&self) -> Option<u64> {
        Some(900)
    }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        let raw_video_url = args["video_url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: video_url"))?;

        // Resolve video URL (SSRF + path traversal guard + CR-01 size-gate)
        // BEFORE the cap check, so a bad input fails fast without consuming
        // a slot. skip_ssrf=true only when client has allow_loopback_cdn set
        // (test-only path).
        let video_url = Self::resolve_video_url(
            raw_video_url,
            self.client.allow_loopback(),
            self.config.video_gen.max_inline_bytes,
        )
        .await?;

        // D-08: model override per call; otherwise the configured v2v
        // default (D-01/D-03).
        let effective_model = args["model"]
            .as_str()
            .filter(|m| !m.is_empty())
            .map(|m| m.to_string())
            .unwrap_or_else(|| self.config.video_gen.v2v.model.clone());

        // D-06 / WR-01: atomically reserve a per-session slot BEFORE any
        // provider call. The guard holds the reservation for the whole
        // (multi-minute, paid) render; committed only on success.
        let slot = match self.try_reserve_slot() {
            Ok(slot) => slot,
            Err(blocked) => return Err(anyhow::anyhow!(blocked)),
        };

        // GEN-05: the shared guardrail chokepoint runs BEFORE any provider
        // call too. A `Root` reservation always succeeds here (D-08 —
        // bounded only by the session_cap check above).
        if let Some(guardrail) = &self.guardrail {
            guardrail
                .try_reserve(&self.reservation_kind, "v2v", &effective_model)
                .map_err(|block| anyhow::anyhow!(block.message()))?;
        }

        // D-02: resolve fal vs venice from the effective model + configured
        // provider — NEVER from the config default alone.
        let backend = gen_backend::resolve(
            self.config.video_gen.v2v.provider.as_deref(),
            &effective_model,
        )?;

        let timeout = Duration::from_secs(self.config.video_gen.timeout_secs);
        let started = std::time::Instant::now();

        self.emit_interim_ack();

        let prompt = args["prompt"].as_str().unwrap_or("").to_string();

        let (outcome, request_id) = match backend {
            GenBackend::Fal(_) => {
                // SAFE-01: lazy FAL_KEY resolution.
                let fal_key = std::env::var("FAL_KEY")
                    .map_err(|_| anyhow::anyhow!("FAL_KEY environment variable not set"))?;

                let body = json!({
                    "video_url": video_url,
                    "prompt": prompt,
                    "duration": self.config.video_gen.default_duration_secs,
                    "resolution": "1080p",
                    "aspect_ratio": "auto",
                    "fps": 25
                });

                let submit = self
                    .client
                    .submit_with_body(&fal_key, &effective_model, body)
                    .await?;
                let request_id = submit.request_id.clone();

                self.client
                    .poll(&fal_key, &submit, timeout)
                    .await
                    .map_err(|e| anyhow::anyhow!("video rendering timed out: {e}"))?;

                let result = self.client.fetch_video(&fal_key, &submit).await?;

                let outcome = self
                    .client
                    .download_video_to_cache_with_cap(
                        &result.video.url,
                        &result.video.content_type,
                        self.config.video_gen.max_inline_bytes,
                    )
                    .await?;
                (outcome, request_id)
            }
            GenBackend::Venice(_) => {
                // SAFE-01: resolve the key lazily, inside execute() — never at boot.
                let venice_key = std::env::var("VENICE_API_KEY")
                    .map_err(|_| anyhow::anyhow!("VENICE_API_KEY environment variable not set"))?;

                // D-13: fetch the model catalog and resolve the CONFIGURED (not
                // LLM-tunable) duration/resolution/aspect_ratio against THIS
                // model's constraints BEFORE any queue call. Fail-closed lives in
                // the model lookup; a catalogued model's empty constraint list
                // means "model-determined" → omit that param. Video-to-video
                // models expose duration as `["Auto"]` (the length follows the
                // source clip), which the duration resolver honours instead of
                // the config's integer seconds.
                let entries = self
                    .venice_client
                    .fetch_models(&venice_key, "video")
                    .await
                    .map_err(|e| venice_video_error(e, timeout))?;
                let constraints =
                    video_constraints_for_model(entries, &effective_model, "video_gen.v2v")?;

                let cfg = &self.config.video_gen;
                let duration = VeniceClient::resolve_video_duration(
                    cfg.default_duration_secs,
                    &constraints.durations,
                    &effective_model,
                    "video_gen.v2v.duration_secs",
                )
                .map_err(|msg| anyhow::anyhow!(msg))?;
                let resolution = VeniceClient::resolve_video_param(
                    "resolution",
                    &cfg.resolution,
                    &constraints.resolutions,
                    &effective_model,
                    "video_gen.v2v.resolution",
                )
                .map_err(|msg| anyhow::anyhow!(msg))?;
                let aspect_ratio = VeniceClient::resolve_video_param(
                    "aspect_ratio",
                    &cfg.aspect_ratio,
                    &constraints.aspect_ratios,
                    &effective_model,
                    "video_gen.v2v.aspect_ratio",
                )
                .map_err(|msg| anyhow::anyhow!(msg))?;

                // D-12: resolution/aspect_ratio/duration come from config,
                // never from LLM args — params the model does not constrain are
                // omitted rather than sent blank.
                let mut body = json!({
                    "model": &effective_model,
                    "video_url": video_url,
                    "prompt": prompt,
                });
                if let Some(d) = &duration {
                    body["duration"] = json!(d);
                }
                if let Some(r) = &resolution {
                    body["resolution"] = json!(r);
                }
                if let Some(a) = &aspect_ratio {
                    body["aspect_ratio"] = json!(a);
                }

                let job = self
                    .venice_client
                    .queue_video(&venice_key, body)
                    .await
                    .map_err(|e| venice_video_error(e, timeout))?;

                let bytes = retrieve_video_with_progress_ping(
                    &self.venice_client,
                    &venice_key,
                    &job,
                    timeout,
                    self.config.video_gen.progress_ping_secs,
                    self.ack_sink.clone(),
                )
                .await
                .map_err(|e| venice_video_error(e, timeout))?;

                let outcome = gen_backend::write_bytes_to_video_cache(
                    &bytes,
                    "mp4",
                    self.config.video_gen.max_inline_bytes,
                )
                .await?;
                (outcome, job.queue_id.clone())
            }
        };

        // WR-01: commit the reservation — the in-flight slot becomes a
        // permanent success and is NOT released on drop.
        slot.commit();
        if let Some(guardrail) = &self.guardrail {
            guardrail.record_success(&self.reservation_kind, "v2v", &effective_model);
        }

        // D-09: raw tracing — no prompt body, no key, no cost.
        debug!(
            model = %effective_model,
            latency_ms = started.elapsed().as_millis() as u64,
            request_id = %request_id,
            "video_to_video completed"
        );

        // Plan 08 (D-10): best-effort artifact-gallery capture for a
        // delegate child (see `image_gen.rs`'s `ArtifactCaptureSink` doc for
        // why this is the ONLY delivery path inside an isolated child).
        if let Some(sink) = &self.capture_sink {
            let media_path = match &outcome {
                DownloadOutcome::Video(p) | DownloadOutcome::OversizedDocument(p) => {
                    p.display().to_string()
                }
                DownloadOutcome::Photo(p) => p.display().to_string(),
            };
            sink.capture("video_to_video", &media_path, raw_video_url);
        }

        match outcome {
            DownloadOutcome::Video(abs_path) => Ok(format!(
                "Generated your video.\n<MEDIA: {}>",
                abs_path.display()
            )),
            // WR-02: emit a `<MEDIA: ...>` tag — identical to the sibling
            // video tools; a video is a video regardless of producing mode.
            DownloadOutcome::OversizedDocument(abs_path) => Ok(format!(
                "Generated your video, but it is large; sending it now.\n<MEDIA: {}>",
                abs_path.display()
            )),
            DownloadOutcome::Photo(_) => unreachable!(
                "download_video_to_cache cannot produce Photo; only download_to_cache does"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
//
// Module names chosen so the plan's verify-command substring filters match:
// `video_to_video::schema` -> `schema_tests`, `video_to_video::availability`
// -> `availability_tests`, `video_to_video::resolve_video_url` ->
// `resolve_video_url_tests` (mirrors the `gen_guardrail.rs` precedent).
// ---------------------------------------------------------------------------

#[cfg(test)]
fn tool() -> VideoToVideoTool {
    VideoToVideoTool::new(Arc::new(Config::default()), FalClient::new())
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    #[test]
    fn name_is_video_to_video() {
        assert_eq!(tool().name(), "video_to_video");
    }

    #[test]
    fn toolset_is_web() {
        assert_eq!(tool().toolset(), "web");
    }

    #[test]
    fn schema_exposes_video_url_prompt_and_model_only() {
        let schema = tool().schema();
        let props = schema
            .function
            .parameters
            .get("properties")
            .and_then(|p| p.as_object())
            .expect("schema must have properties");
        assert!(props.contains_key("video_url"));
        assert!(props.contains_key("prompt"));
        assert!(props.contains_key("model"));
        // D-12: duration/resolution/aspect are config-fixed, NOT LLM-tunable.
        assert!(!props.contains_key("duration"));
        assert!(!props.contains_key("resolution"));
        assert!(!props.contains_key("aspect_ratio"));
        // D-08: config-only knobs are NOT in the schema.
        assert!(!props.contains_key("session_cap"));
        assert!(!props.contains_key("timeout_secs"));

        let required = schema
            .function
            .parameters
            .get("required")
            .and_then(|r| r.as_array())
            .expect("schema must have required array");
        assert!(required.iter().any(|v| v == "video_url"));
    }
}

#[cfg(test)]
mod availability_tests {
    use super::*;

    /// Serialized to avoid env races with other tests reading these keys.
    static KEY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn is_available_gated_on_venice_key_for_the_default_config() {
        let _g = KEY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prior = std::env::var("VENICE_API_KEY").ok();
        unsafe {
            std::env::remove_var("VENICE_API_KEY");
        }
        assert!(
            !tool().is_available(),
            "video_to_video must be hidden when VENICE_API_KEY is unset (D-03 venice default)"
        );
        unsafe {
            std::env::set_var("VENICE_API_KEY", "test-key-not-real");
        }
        assert!(
            tool().is_available(),
            "video_to_video must be available when VENICE_API_KEY is set"
        );
        unsafe {
            match prior {
                Some(v) => std::env::set_var("VENICE_API_KEY", v),
                None => std::env::remove_var("VENICE_API_KEY"),
            }
        }
    }

    #[test]
    fn is_available_gated_on_fal_key_when_explicitly_configured_to_a_fal_model() {
        let _g = KEY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut config = Config::default();
        config.video_gen.v2v.provider = Some("fal".to_string());
        config.video_gen.v2v.model = "fal-ai/some-v2v-model".to_string();
        let tool = VideoToVideoTool::new(Arc::new(config), FalClient::new());

        let prior = std::env::var("FAL_KEY").ok();
        unsafe {
            std::env::remove_var("FAL_KEY");
        }
        assert!(
            !tool.is_available(),
            "must be hidden when configured for fal and FAL_KEY is unset"
        );
        unsafe {
            std::env::set_var("FAL_KEY", "test-key-not-real");
        }
        assert!(
            tool.is_available(),
            "must be available when configured for fal and FAL_KEY is set"
        );
        unsafe {
            match prior {
                Some(v) => std::env::set_var("FAL_KEY", v),
                None => std::env::remove_var("FAL_KEY"),
            }
        }
    }

    #[test]
    fn prerequisites_declare_both_provider_keys() {
        let prereqs = tool().prerequisites();
        assert_eq!(prereqs.len(), 2);
        assert!(prereqs.iter().any(|p| p.name == "VENICE_API_KEY"));
        assert!(prereqs.iter().any(|p| p.name == "FAL_KEY"));
        assert!(prereqs.iter().all(|p| p.kind == "env_var"));
    }
}

#[cfg(test)]
mod resolve_video_url_tests {
    use super::*;
    use ironhermes_core::constants::get_hermes_home;
    use tempfile::TempDir;

    /// Serialize IRONHERMES_HOME env mutation across tests in this module
    /// (parallel-safe) — mirrors the established `video_gen.rs` pattern.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct HermesHomeGuard {
        _tmp: TempDir,
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl HermesHomeGuard {
        fn new() -> Self {
            let lock = HOME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let tmp = TempDir::new().expect("create tempdir");
            let prev = std::env::var("IRONHERMES_HOME").ok();
            // Canonicalize the tempdir path UP FRONT (e.g. macOS's `/var` is a
            // symlink to `/private/var`) so it matches what
            // `resolve_video_url`'s `tokio::fs::canonicalize` on a file inside
            // it will resolve to — otherwise the `starts_with(cache_root)`
            // prefix check spuriously fails on platforms with a symlinked tmp
            // root.
            let canonical_tmp = std::fs::canonicalize(tmp.path()).expect("canonicalize tempdir");
            unsafe {
                std::env::set_var("IRONHERMES_HOME", &canonical_tmp);
            }
            Self {
                _tmp: tmp,
                prev,
                _lock: lock,
            }
        }
    }

    impl Drop for HermesHomeGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                    None => std::env::remove_var("IRONHERMES_HOME"),
                }
            }
        }
    }

    #[tokio::test]
    async fn rejects_data_uri() {
        let err = VideoToVideoTool::resolve_video_url(
            "data:video/mp4;base64,AAAA",
            false,
            10 * 1024 * 1024,
        )
        .await
        .expect_err("a data: URI must be rejected, not accepted");
        let msg = err.to_string();
        assert!(
            msg.contains("inline video data") && msg.contains("path"),
            "error should mention inline video data and a path, got: {msg}"
        );
    }

    #[tokio::test]
    async fn safe_url_passes_through_unchanged() {
        let url = VideoToVideoTool::resolve_video_url(
            "https://example.com/clip.mp4",
            true, // skip_ssrf: test-only bypass
            10 * 1024 * 1024,
        )
        .await
        .expect("a safe URL (SSRF-skip) must pass through");
        assert_eq!(url, "https://example.com/clip.mp4");
    }

    #[tokio::test]
    async fn unsafe_url_rejected_by_ssrf_check() {
        // A loopback/private-range URL fails is_safe_url when skip_ssrf=false.
        let err = VideoToVideoTool::resolve_video_url(
            "http://127.0.0.1:1/clip.mp4",
            false,
            10 * 1024 * 1024,
        )
        .await
        .expect_err("a loopback URL must be rejected by the SSRF check");
        assert!(
            err.to_string().contains("SSRF"),
            "error should mention SSRF, got: {err}"
        );
    }

    #[tokio::test]
    async fn local_path_outside_cache_root_is_rejected_after_canonicalize() {
        let _home = HermesHomeGuard::new();
        // A real file that exists but lives OUTSIDE $IRONHERMES_HOME/cache/.
        let outside = TempDir::new().expect("create outside tempdir");
        let file_path = outside.path().join("evil.mp4");
        tokio::fs::write(&file_path, b"not really a video")
            .await
            .expect("write outside file");

        let err = VideoToVideoTool::resolve_video_url(
            file_path.to_str().expect("valid utf8 path"),
            false,
            10 * 1024 * 1024,
        )
        .await
        .expect_err("a path outside the cache root must be rejected");
        assert!(
            err.to_string().contains("cache root"),
            "error should mention the cache root, got: {err}"
        );
    }

    #[tokio::test]
    async fn oversized_local_file_rejected_via_metadata_before_read() {
        let _home = HermesHomeGuard::new();
        let cache_dir = get_hermes_home().join("cache");
        tokio::fs::create_dir_all(&cache_dir)
            .await
            .expect("create cache dir");
        let file_path = cache_dir.join("huge.mp4");
        // A file whose size (100 bytes) exceeds a tiny 10-byte cap — the
        // metadata check must reject it before any `tokio::fs::read` call.
        tokio::fs::write(&file_path, vec![0u8; 100])
            .await
            .expect("write oversized file");

        let err = VideoToVideoTool::resolve_video_url(
            file_path.to_str().expect("valid utf8 path"),
            false,
            10, // max_inline_bytes cap smaller than the file
        )
        .await
        .expect_err("an oversized file must be rejected via the size-gate");
        assert!(
            err.to_string().contains("too large to inline"),
            "error should mention the size cap, got: {err}"
        );
    }

    #[tokio::test]
    async fn in_cache_video_path_is_base64_encoded_as_data_uri() {
        let _home = HermesHomeGuard::new();
        let cache_dir = get_hermes_home().join("cache");
        tokio::fs::create_dir_all(&cache_dir)
            .await
            .expect("create cache dir");
        let file_path = cache_dir.join("clip.mp4");
        tokio::fs::write(&file_path, b"fake mp4 bytes")
            .await
            .expect("write cache file");

        let uri = VideoToVideoTool::resolve_video_url(
            file_path.to_str().expect("valid utf8 path"),
            false,
            10 * 1024 * 1024,
        )
        .await
        .expect("an in-cache video path must be accepted");
        assert!(
            uri.starts_with("data:video/mp4;base64,"),
            "must encode as a video/mp4 data URI, got: {uri}"
        );
    }

    #[test]
    fn video_mime_from_extension_returns_correct_types() {
        assert_eq!(video_mime_from_extension(Some("mp4")), "video/mp4");
        assert_eq!(video_mime_from_extension(Some("mov")), "video/quicktime");
        assert_eq!(video_mime_from_extension(Some("webm")), "video/webm");
        assert_eq!(video_mime_from_extension(None), "application/octet-stream");
        assert_eq!(
            video_mime_from_extension(Some("xyz")),
            "application/octet-stream"
        );
    }
}

#[cfg(test)]
mod execute_tests {
    use super::*;
    use ironhermes_core::types::Platform;
    use std::collections::HashMap;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Serialize IRONHERMES_HOME env mutation across tests in this module
    /// (parallel-safe) — mirrors the established `video_gen.rs` pattern.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct HermesHomeGuard {
        _tmp: tempfile::TempDir,
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl HermesHomeGuard {
        fn new() -> Self {
            let lock = HOME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let tmp = tempfile::TempDir::new().expect("create tempdir");
            let prev = std::env::var("IRONHERMES_HOME").ok();
            unsafe {
                std::env::set_var("IRONHERMES_HOME", tmp.path());
            }
            Self {
                _tmp: tmp,
                prev,
                _lock: lock,
            }
        }
    }

    impl Drop for HermesHomeGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                    None => std::env::remove_var("IRONHERMES_HOME"),
                }
            }
        }
    }

    fn set_venice_key(key: &str) -> Option<String> {
        let prior = std::env::var("VENICE_API_KEY").ok();
        unsafe {
            std::env::set_var("VENICE_API_KEY", key);
        }
        prior
    }

    fn restore_venice_key(prior: Option<String>) {
        unsafe {
            match prior {
                Some(v) => std::env::set_var("VENICE_API_KEY", v),
                None => std::env::remove_var("VENICE_API_KEY"),
            }
        }
    }

    /// Mount a `GET /models?type=video` catalog whose single entry matches
    /// `model` and whose constraints allow exactly `config.video_gen`'s
    /// `resolution`/`aspect_ratio`/`default_duration_secs`.
    async fn mount_matching_video_model_catalog(server: &MockServer, model: &str, cfg: &Config) {
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{
                    "id": model,
                    "model_spec": {
                        "constraints": {
                            "aspect_ratios": [cfg.video_gen.aspect_ratio.clone()],
                            "resolutions": [cfg.video_gen.resolution.clone()],
                            "durations": [format!("{}s", cfg.video_gen.default_duration_secs)],
                            "model_type": "video-to-video",
                            "audio": false,
                            "audio_configurable": false,
                        }
                    }
                }]
            })))
            .mount(server)
            .await;
    }

    /// A venice v2v model validates duration/resolution/aspect against the
    /// model_spec, queues (with `video_url` in the body), and returns bytes
    /// on COMPLETED (mocked) — the unchanged `<MEDIA:>` video tag is emitted.
    #[tokio::test]
    async fn venice_v2v_happy_path_dispatches_validates_and_emits_media_tag() {
        let _home = HermesHomeGuard::new();
        let server = MockServer::start().await;
        let model = "wan-2-7-video-to-video";
        let config = Config::default(); // v2v defaults to venice/wan-2-7-video-to-video

        mount_matching_video_model_catalog(&server, model, &config).await;

        Mock::given(method("POST"))
            .and(path("/video/queue"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": model,
                "queue_id": "q-v2v-1",
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(vec![0u8; 32], "video/mp4"))
            .mount(&server)
            .await;

        let tool = VideoToVideoTool::new(
            Arc::new(config),
            FalClient::with_base_url_allow_loopback(server.uri()),
        )
        .with_venice_client(VeniceClient::with_base_url(server.uri()));

        let prior = set_venice_key("test-venice-key-not-real");
        let result = tool
            .execute(json!({"video_url": format!("{}/input.mp4", server.uri())}))
            .await;
        restore_venice_key(prior);

        let output = result.expect("venice v2v dispatch must succeed");
        assert!(
            output.starts_with("Generated your video.\n<MEDIA: "),
            "must emit the unchanged video MEDIA tag, got: {output}"
        );
        assert!(
            output.contains(".mp4"),
            "must reference an .mp4 file, got: {output}"
        );
    }

    /// D-13: a configured duration outside the model's allowed set is
    /// rejected PRE-call with the exact template — no `queue_video` call.
    #[tokio::test]
    async fn venice_v2v_out_of_range_duration_rejected_pre_call_no_queue_call() {
        let server = MockServer::start().await;
        let model = "wan-2-7-video-to-video";

        let mut config = Config::default();
        config.video_gen.v2v.provider = Some("venice".to_string());
        config.video_gen.v2v.model = model.to_string();
        // The catalog only allows duration "3"/"5"/"8"; default_duration_secs
        // (6) is out of range.
        config.video_gen.default_duration_secs = 6;
        config.video_gen.resolution = "720p".to_string();
        config.video_gen.aspect_ratio = "16:9".to_string();

        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{
                    "id": model,
                    "model_spec": {
                        "constraints": {
                            "aspect_ratios": ["16:9"],
                            "resolutions": ["720p"],
                            "durations": ["3s", "5s", "8s"],
                            "model_type": "video-to-video",
                        }
                    }
                }]
            })))
            .mount(&server)
            .await;
        // Deliberately NO /video/queue mock — asserted unreached below.

        let tool = VideoToVideoTool::new(
            Arc::new(config),
            FalClient::with_base_url_allow_loopback(server.uri()),
        )
        .with_venice_client(VeniceClient::with_base_url(server.uri()));

        let prior = set_venice_key("k");
        let err = tool
            .execute(json!({"video_url": format!("{}/input.mp4", server.uri())}))
            .await
            .expect_err("out-of-range duration must be rejected pre-call");
        restore_venice_key(prior);

        let msg = err.to_string();
        assert_eq!(
            msg,
            "duration 6s not supported by wan-2-7-video-to-video; allowed: 3s, 5s, 8s. Fix video_gen.v2v.duration_secs."
        );

        let requests = server
            .received_requests()
            .await
            .expect("wiremock request log must be available");
        assert!(
            !requests.iter().any(|r| r.url.path() == "/video/queue"),
            "must not call queue_video on a pre-call D-13 rejection"
        );
    }

    /// GEN-05 ordering: a guardrail block returns the non-retried message
    /// with NO provider call at all (not even the model catalog fetch).
    #[tokio::test]
    async fn guardrail_block_returns_non_retried_message_with_no_provider_call() {
        let server = MockServer::start().await;
        let config = Config::default();

        // pool=0 blocks the very first Descendant reservation.
        let guardrail = Arc::new(GenerationGuardrail::new(0, 10, "test-root"));
        let tool = VideoToVideoTool::new(
            Arc::new(config),
            FalClient::with_base_url_allow_loopback(server.uri()),
        )
        .with_venice_client(VeniceClient::with_base_url(server.uri()))
        .with_guardrail(
            guardrail,
            ReservationKind::Descendant {
                child_id: "child-1".to_string(),
            },
        );

        let prior = set_venice_key("k");
        let err = tool
            .execute(json!({"video_url": format!("{}/input.mp4", server.uri())}))
            .await
            .expect_err("guardrail must block before any provider call");
        restore_venice_key(prior);

        let msg = err.to_string();
        assert!(
            msg.contains("Generation pool exhausted"),
            "must surface the non-retried guardrail message: {msg}"
        );
        assert!(
            !msg.to_lowercase().contains("retry") && !msg.to_lowercase().contains("try again"),
            "must be non-retried: {msg}"
        );

        let requests = server
            .received_requests()
            .await
            .expect("wiremock request log must be available");
        assert!(
            requests.is_empty(),
            "guardrail block must happen before any provider call, got: {requests:?}"
        );
    }

    /// A Venice job failure (non-2xx mid-poll) surfaces the exact non-retried
    /// "Video generation failed: {reason}." template.
    #[tokio::test]
    async fn venice_v2v_job_failure_surfaces_non_retried_message() {
        let _home = HermesHomeGuard::new();
        let server = MockServer::start().await;
        let model = "wan-2-7-video-to-video";
        let config = Config::default();
        mount_matching_video_model_catalog(&server, model, &config).await;

        Mock::given(method("POST"))
            .and(path("/video/queue"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": model,
                "queue_id": "q-fail",
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let tool = VideoToVideoTool::new(
            Arc::new(config),
            FalClient::with_base_url_allow_loopback(server.uri()),
        )
        .with_venice_client(VeniceClient::with_base_url(server.uri()));

        let prior = set_venice_key("k");
        let err = tool
            .execute(json!({"video_url": format!("{}/input.mp4", server.uri())}))
            .await
            .expect_err("a mid-poll job failure must surface as an error");
        restore_venice_key(prior);

        let msg = err.to_string();
        assert!(
            msg.starts_with("Video generation failed: "),
            "must use the exact non-retried failure template, got: {msg}"
        );
    }

    /// A poll timeout surfaces the exact non-retried
    /// "Video generation timed out after {timeout}s." template.
    #[tokio::test]
    async fn venice_v2v_timeout_surfaces_non_retried_message() {
        let _home = HermesHomeGuard::new();
        let server = MockServer::start().await;
        let model = "wan-2-7-video-to-video";
        let mut config = Config::default();
        // Instant timeout: retrieve_video's deadline check trips on the very
        // first loop iteration, before any HTTP request.
        config.video_gen.timeout_secs = 0;
        mount_matching_video_model_catalog(&server, model, &config).await;

        Mock::given(method("POST"))
            .and(path("/video/queue"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": model,
                "queue_id": "q-timeout",
            })))
            .mount(&server)
            .await;

        let tool = VideoToVideoTool::new(
            Arc::new(config),
            FalClient::with_base_url_allow_loopback(server.uri()),
        )
        .with_venice_client(VeniceClient::with_base_url(server.uri()));

        let prior = set_venice_key("k");
        let err = tool
            .execute(json!({"video_url": format!("{}/input.mp4", server.uri())}))
            .await
            .expect_err("a poll timeout must surface as an error");
        restore_venice_key(prior);

        assert_eq!(err.to_string(), "Video generation timed out after 0s.");
    }

    /// UI-SPEC: the interim ack is the exact shipped video ack string
    /// verbatim — no v2v-specific variant.
    #[tokio::test]
    async fn interim_ack_matches_shipped_video_ack_string_verbatim() {
        let _home = HermesHomeGuard::new();
        let server = MockServer::start().await;
        let model = "wan-2-7-video-to-video";
        let config = Config::default();
        mount_matching_video_model_catalog(&server, model, &config).await;

        Mock::given(method("POST"))
            .and(path("/video/queue"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": model,
                "queue_id": "q-ack",
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(vec![0u8; 16], "video/mp4"))
            .mount(&server)
            .await;

        struct RecordingAckSink {
            messages: std::sync::Mutex<Vec<String>>,
        }
        impl AckSink for RecordingAckSink {
            fn ack(&self, message: &str) {
                self.messages
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(message.to_string());
            }
        }

        let sink = Arc::new(RecordingAckSink {
            messages: std::sync::Mutex::new(Vec::new()),
        });
        let counter: VideoSessionCounter = Arc::new(Mutex::new(HashMap::new()));
        let session_key = SessionKey::new(Platform::Local, "v2v-ack-test");
        let tool = VideoToVideoTool::new_with_session(
            session_key,
            Arc::new(config),
            FalClient::with_base_url_allow_loopback(server.uri()),
            counter,
            Some(sink.clone()),
        )
        .with_venice_client(VeniceClient::with_base_url(server.uri()));

        let prior = set_venice_key("k");
        let result = tool
            .execute(json!({"video_url": format!("{}/input.mp4", server.uri())}))
            .await;
        restore_venice_key(prior);
        result.expect("venice v2v dispatch must succeed");

        let messages = sink.messages.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            messages
                .iter()
                .any(|m| m == "Generating your video… this may take a few minutes."),
            "must emit the exact shipped video interim ack verbatim, got: {messages:?}"
        );
    }
}

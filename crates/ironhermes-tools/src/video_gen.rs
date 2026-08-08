//! Phase 36.3.3 Plan 02 — `video_gen` LLM tools: `VideoGenerateTool` (T2V) and
//! `VideoAnimateTool` (I2V).
//!
//! Both tools follow the same fal.ai queue pipeline used by `image_gen`:
//! submit → poll → fetch → download → `<MEDIA:>` tag. Key deltas from image_gen:
//!
//! * **T2V body** — `prompt`, `duration`, `resolution`, `aspect_ratio`, `fps`
//!   (NOT `num_inference_steps` — that is an image field). Default model:
//!   `fal-ai/ltx-2.3/text-to-video` (D-10).
//!
//! * **I2V body** — adds `image_url` field. Accepts either:
//!   - A public HTTPS URL validated by `is_safe_url` (passed unchanged), or
//!   - A local path clamped under `$IRONHERMES_HOME/cache/` — reads bytes, encodes
//!     to a `data:<mime>;base64,<b64>` URI (D-02).
//!
//! * **Session cap** — 5 (D-06, paid video). Separate `VideoSessionCounter` to
//!   keep video and image cap counts independent (Pitfall 8).
//!
//! * **Timeout** — 300 s (D-04). The `poll()` timeout error is re-wrapped to say
//!   "video rendering timed out" (not "image generation timed out" — Pitfall 7).
//!
//! * **Size cap** — 50 MB (D-07). The `download_video_to_cache` function enforces
//!   this; we pass through `DownloadOutcome::Video` or `OversizedDocument`.
//!
//! * **D-12** — FAL_KEY unset → tools hidden from schema (same Prerequisite pattern
//!   as image_gen).
//!
//! * **D-09** — raw tracing after success (model, latency, request_id).
//!
//! Phase 47 Plan 06 rewires both tools' `execute()` to dispatch through
//! [`crate::gen_backend`] (fal or venice per the *effective* per-mode
//! `video_gen.{t2v,i2v}.model`, D-01/D-02) and adds the shared
//! [`GenerationGuardrail`] chokepoint (Plan 05, GEN-05) BEFORE any provider
//! call. The Venice arm threads `resolution`/`aspect_ratio`/
//! `default_duration_secs` from config (D-12, fixed, not LLM-tunable),
//! fail-closed-validates them against the model's catalog constraints before
//! queueing (D-13), and surfaces a distinct non-retried failure/timeout string
//! (UI-SPEC) instead of ever hanging or returning a silent empty result. The
//! fal arm, the session cap, and the `<MEDIA:>` emit strings are unchanged.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ironhermes_core::{Config, SessionKey, ToolSchema};
use serde_json::{Value, json};
use tracing::debug;

use base64::Engine as _; // CRITICAL: must be in scope for .encode() on STANDARD engine

use crate::fal::{DownloadOutcome, FalClient};
use crate::gen_backend::{self, GenBackend};
use crate::gen_guardrail::{GenerationGuardrail, ReservationKind};
use crate::image_gen::{AckSink, ArtifactCaptureSink};
use crate::registry::Tool;
use crate::venice::{QueuedVideoJob, VeniceClient, video_constraints_for_model};

/// Map a Venice video-path error to the exact UI-SPEC non-retried strings
/// (D-13/UI-SPEC). Venice's `/video/retrieve` schema has no `FAILED` status
/// (RESEARCH Pitfall 3) — the ONLY Venice error whose message contains "timed
/// out" is the poll-deadline error `retrieve_video` returns, so that substring
/// is the sole discriminant between the two templates. Every other failure
/// (queue submit, mid-poll non-2xx, model-catalog fetch, decode) surfaces as
/// the generic failure template — never a silent empty result, never a hang.
fn venice_video_error(e: anyhow::Error, timeout: Duration) -> anyhow::Error {
    let msg = e.to_string();
    if msg.contains("timed out") {
        anyhow::anyhow!("Video generation timed out after {}s.", timeout.as_secs())
    } else {
        anyhow::anyhow!("Video generation failed: {msg}.")
    }
}

/// Config-gated periodic "Still working on your video…" progress ping (Phase
/// 47 UI-SPEC; backfills 36.3.3's never-shipped D-05). Rides the EXISTING
/// [`AckSink`] channel wired to `retrieve_video`'s `on_tick` hook — no new
/// iron_hermes_ui WS event (RESEARCH Pitfall 6). A `cadence` of zero is a
/// permanent no-op: only the single up-front interim ack (already emitted by
/// `emit_interim_ack` before polling starts) is ever sent — today's behavior.
struct ProgressPinger {
    ack_sink: Option<Arc<dyn AckSink>>,
    cadence: Duration,
    /// Seeded to "now" at construction (polling start), not `None` — so the
    /// first re-ping only fires once a FULL cadence has elapsed since polling
    /// began, not on the very first `on_tick` call (which fires before any
    /// wait, right after the up-front interim ack).
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

    /// Called once per `retrieve_video` poll iteration via `on_tick`. A no-op
    /// when `cadence` is zero (off) or no `AckSink` is wired; otherwise
    /// re-acks only once at least `cadence` has elapsed since the last ping.
    fn tick(&self) {
        if self.cadence.is_zero() {
            return;
        }
        let Some(sink) = &self.ack_sink else {
            return;
        };
        let now = std::time::Instant::now();
        let mut last = self
            .last_ping
            .lock()
            .expect("progress pinger mutex poisoned");
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
/// progress ping wired to its `on_tick` hook (Task 3). Shared by both
/// `VideoGenerateTool` (t2v) and `VideoAnimateTool` (i2v) — the ping copy and
/// gating are identical for both modes (UI-SPEC: no mode-specific variant).
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

/// Shared, `SessionKey`-keyed video generation counter.
///
/// Separate from `image_gen::SessionCounter` so the video cap (5 per session,
/// D-06) never interferes with the image cap (Pitfall 8 prevention).
pub type VideoSessionCounter = Arc<Mutex<HashMap<SessionKey, u32>>>;

/// RAII reservation for one in-flight video generation slot (WR-01).
///
/// Returned by the shared `reserve_video_slot` helper. The slot is *reserved*
/// (the counter is incremented) the moment the guard is created, under the same
/// lock scope that performed the cap check — closing the TOCTOU/concurrency hole
/// where N concurrent `execute` calls could all observe `used < cap` and all
/// proceed. The reservation is released (decremented) on `Drop` unless
/// [`SlotGuard::commit`] is called, which converts the in-flight reservation
/// into a permanent success.
///
/// A stateless tool (no session key / counter) yields an inert guard that holds
/// no reservation and is a no-op on both commit and drop.
#[must_use = "the reservation is released when the guard is dropped; hold it for the lifetime of the generation"]
pub struct SlotGuard {
    /// `None` for the stateless (no-cap) path — nothing to release or commit.
    inner: Option<(SessionKey, VideoSessionCounter)>,
    committed: bool,
}

impl SlotGuard {
    /// Inert guard for stateless tools (no cap enforced).
    fn inert() -> Self {
        Self {
            inner: None,
            committed: true, // nothing reserved; nothing to release on drop
        }
    }

    /// Commit the reservation: the in-flight slot becomes a recorded success and
    /// will NOT be decremented on drop.
    pub fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Some((key, counter)) = &self.inner {
            // Release the in-flight reservation. Recover from poison so a single
            // panic elsewhere cannot brick the shared counter (IN-03 spirit).
            let mut guard = counter.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(slot) = guard.get_mut(key) {
                *slot = slot.saturating_sub(1);
            }
        }
    }
}

/// Atomically check the per-session cap and reserve a slot in a single lock
/// scope (WR-01). Returns an RAII [`SlotGuard`] on success; the reservation is
/// already counted toward the cap when this returns `Ok`. Returns `Err` with a
/// non-retried message once the session has reached `cap` reserved-or-completed
/// generations.
///
/// A stateless caller (no session key / counter) always succeeds with an inert
/// guard.
fn reserve_video_slot(
    session_key: &Option<SessionKey>,
    counter: &Option<VideoSessionCounter>,
    cap: u32,
) -> Result<SlotGuard, String> {
    let (Some(key), Some(counter)) = (session_key, counter) else {
        return Ok(SlotGuard::inert());
    };
    // Single lock scope: check-and-increment atomically (closes the TOCTOU and
    // the concurrent-in-flight hole). Poison-tolerant (IN-03 spirit).
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
    Ok(SlotGuard {
        inner: Some((key.clone(), counter.clone())),
        committed: false,
    })
}

// ---------------------------------------------------------------------------
// VideoGenerateTool — text-to-video (T2V)
// ---------------------------------------------------------------------------

/// Text-to-video tool backed by the fal.ai LTX-2.3 queue API.
///
/// Schema exposes: `prompt` (required), `model` (optional). Config-only knobs
/// (`session_cap`, `timeout_secs`) are intentionally absent from the schema (D-08).
pub struct VideoGenerateTool {
    config: Arc<Config>,
    client: FalClient,
    /// Venice.ai client (Phase 47 D-01/D-02). Defaults to production
    /// `VeniceClient::new()`; overridable via [`VideoGenerateTool::with_venice_client`].
    venice_client: VeniceClient,
    session_key: Option<SessionKey>,
    counter: Option<VideoSessionCounter>,
    ack_sink: Option<Arc<dyn AckSink>>,
    /// Shared spend guardrail (Phase 47 Plan 05, GEN-05). `None` = not
    /// enforced (pre-Plan-08 call sites) — the per-session cap still applies.
    guardrail: Option<Arc<GenerationGuardrail>>,
    /// D-08: `Root` (direct chat, exempt from `per_child_cap`/the descendant
    /// pool) or `Descendant` (delegate/kanban, subject to both tiers).
    /// Defaults to `Root`; wired by Plan 08 at production construction time.
    reservation_kind: ReservationKind,
    /// Plan 08 (D-10): optional artifact-gallery capture sink, wired only for
    /// delegate children.
    capture_sink: Option<Arc<dyn ArtifactCaptureSink>>,
}

impl VideoGenerateTool {
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

    /// Per-session construction (production path). Cap is scoped to `session_key`.
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
    /// need a new required parameter (mirrors `ImageGenTool::with_guardrail`).
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

    /// Cap check + atomic reservation (D-06 / SAFE-05 / WR-01): returns a
    /// [`SlotGuard`] that has *already* reserved (incremented) a slot under the
    /// same lock scope as the cap check, or `Err` with a non-retried message once
    /// the session reaches `config.video_gen.session_cap` reserved-or-completed
    /// generations. The reservation is released on drop unless committed via
    /// [`SlotGuard::commit`] after the generation succeeds — so concurrent
    /// in-flight jobs now count against the cap (no unbounded-cost hole).
    ///
    /// A stateless tool (no session key) always returns an inert guard.
    pub fn try_reserve_slot(&self) -> Result<SlotGuard, String> {
        reserve_video_slot(
            &self.session_key,
            &self.counter,
            self.config.video_gen.session_cap,
        )
    }

    fn emit_interim_ack(&self) {
        if let Some(sink) = &self.ack_sink {
            sink.ack("Generating your video… this may take a few minutes.");
        }
    }

    /// Build the T2V JSON body for the fal submit request.
    fn build_t2v_body(prompt: &str, model: &str, config: &Config) -> Value {
        // The LTX-2.3 T2V schema fields (confirmed against live docs):
        //   prompt, duration, resolution, aspect_ratio, fps
        // Notably ABSENT: num_inference_steps (that is an image/image-gen field).
        let _ = model; // model is passed on URL, not in body
        json!({
            "prompt": prompt,
            "duration": config.video_gen.default_duration_secs,
            "resolution": "1080p",
            "aspect_ratio": "16:9",
            "fps": 25
        })
    }
}

#[async_trait]
impl Tool for VideoGenerateTool {
    fn name(&self) -> &str {
        "video_generate"
    }

    fn toolset(&self) -> &str {
        "web"
    }

    fn description(&self) -> &str {
        "Generate a short video clip from a text prompt and deliver it to the user as a native \
         attachment. Provide a vivid, detailed `prompt`. Optionally override the model via `model`."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "video_generate",
            "Generate a short video clip from a text prompt and deliver it as a native attachment. \
             Provide a vivid, detailed `prompt`.",
            json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "A detailed description of the video to generate."
                    },
                    "model": {
                        "type": "string",
                        "description": "Optional fal model id to override the configured default \
                                        (e.g. \"fal-ai/ltx-2.3/text-to-video\")."
                    }
                },
                "required": ["prompt"]
            }),
        )
    }

    fn prerequisites(&self) -> Vec<crate::registry::Prerequisite> {
        // D-12: hidden from schema when FAL_KEY is unset.
        vec![crate::registry::Prerequisite {
            kind: "env_var".to_string(),
            name: "FAL_KEY".to_string(),
            description: "fal.ai API key — required for video_gen to generate videos.".to_string(),
            required: true,
            group: None,
        }]
    }

    /// D-04 (Phase 41.3): provider-side video generation routinely exceeds
    /// the 60s trait default.
    fn timeout_secs(&self) -> Option<u64> {
        Some(900)
    }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        let prompt = args["prompt"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: prompt"))?;

        // D-08: model override per call; otherwise the configured t2v default
        // (Phase 47 D-01/D-03 — per-mode, replaces the legacy flat
        // `default_t2v_model` read).
        let effective_model = args["model"]
            .as_str()
            .filter(|m| !m.is_empty())
            .map(|m| m.to_string())
            .unwrap_or_else(|| self.config.video_gen.t2v.model.clone());

        // D-06 / WR-01: atomically reserve a per-session slot BEFORE any provider
        // call. The guard holds the reservation for the whole (multi-minute, paid)
        // render so concurrent in-flight jobs all count against the cap; it
        // auto-releases on any early return below and is committed only on success.
        let slot = match self.try_reserve_slot() {
            Ok(slot) => slot,
            Err(blocked) => return Err(anyhow::anyhow!(blocked)),
        };

        // GEN-05 / T-47-04e: the shared guardrail chokepoint runs BEFORE any
        // provider call too. A `Root` reservation always succeeds here (D-08 —
        // bounded only by the session_cap check above).
        if let Some(guardrail) = &self.guardrail {
            guardrail
                .try_reserve(&self.reservation_kind, "t2v", &effective_model)
                .map_err(|block| anyhow::anyhow!(block.message()))?;
        }

        // D-02: resolve fal vs venice from the effective model + configured
        // provider — NEVER from the config default alone (Plan 04 prohibition).
        let backend = gen_backend::resolve(
            self.config.video_gen.t2v.provider.as_deref(),
            &effective_model,
        )?;

        let timeout = Duration::from_secs(self.config.video_gen.timeout_secs);
        let started = std::time::Instant::now();

        self.emit_interim_ack();

        let (outcome, request_id) = match backend {
            GenBackend::Fal(_) => {
                // SAFE-01: resolve FAL_KEY lazily inside execute().
                let fal_key = std::env::var("FAL_KEY")
                    .map_err(|_| anyhow::anyhow!("FAL_KEY environment variable not set"))?;

                let body = Self::build_t2v_body(prompt, &effective_model, &self.config);
                let submit = self
                    .client
                    .submit_with_body(&fal_key, &effective_model, body)
                    .await?;
                let request_id = submit.request_id.clone();

                // D-04: wrap poll timeout to say "video rendering timed out" (Pitfall 7).
                self.client
                    .poll(&fal_key, &submit, timeout)
                    .await
                    .map_err(|e| anyhow::anyhow!("video rendering timed out: {e}"))?;

                let result = self.client.fetch_video(&fal_key, &submit).await?;

                // WR-01: thread the configurable download cap
                // (config.video_gen.max_inline_bytes) instead of relying on the
                // hardcoded VIDEO_SIZE_CAP const default.
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
                // the model lookup — an uncatalogued model is a hard error (a
                // typo can never send an unconstrained request); a catalogued
                // model's empty constraint list means "model-determined" → omit
                // that param. Durations are Venice's `"{n}s"` strings (or
                // `"Auto"`), never the bare integer the config stores.
                let entries = self
                    .venice_client
                    .fetch_models(&venice_key, "video")
                    .await
                    .map_err(|e| venice_video_error(e, timeout))?;
                let constraints =
                    video_constraints_for_model(entries, &effective_model, "video_gen.t2v")?;

                let cfg = &self.config.video_gen;
                let duration = VeniceClient::resolve_video_duration(
                    cfg.default_duration_secs,
                    &constraints.durations,
                    &effective_model,
                    "video_gen.t2v.duration_secs",
                )
                .map_err(|msg| anyhow::anyhow!(msg))?;
                let resolution = VeniceClient::resolve_video_param(
                    "resolution",
                    &cfg.resolution,
                    &constraints.resolutions,
                    &effective_model,
                    "video_gen.t2v.resolution",
                )
                .map_err(|msg| anyhow::anyhow!(msg))?;
                let aspect_ratio = VeniceClient::resolve_video_param(
                    "aspect_ratio",
                    &cfg.aspect_ratio,
                    &constraints.aspect_ratios,
                    &effective_model,
                    "video_gen.t2v.aspect_ratio",
                )
                .map_err(|msg| anyhow::anyhow!(msg))?;

                // D-12: resolution/aspect_ratio/duration come from config, never
                // from LLM args — and a param the model does not constrain is
                // omitted rather than sent blank.
                let mut body = json!({
                    "model": &effective_model,
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

                // Task 3: wire the config-gated periodic progress ping to the
                // retrieve_video on_tick hook, riding the existing AckSink
                // (Pitfall 6 — no new WS event). progress_ping_secs = 0/absent
                // is a no-op (today's single-ack behavior).
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

        // WR-01: commit the reservation — the in-flight slot becomes a permanent
        // success and is NOT released on drop.
        slot.commit();
        if let Some(guardrail) = &self.guardrail {
            guardrail.record_success(&self.reservation_kind, "t2v", &effective_model);
        }

        // D-09: raw tracing — no prompt body, no key, no cost.
        debug!(
            model = %effective_model,
            latency_ms = started.elapsed().as_millis() as u64,
            request_id = %request_id,
            "video_gen completed"
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
            sink.capture("video_generate", &media_path, prompt);
        }

        match outcome {
            DownloadOutcome::Video(abs_path) => Ok(format!(
                "Generated your video.\n<MEDIA: {}>",
                abs_path.display()
            )),
            // WR-02: emit a `<MEDIA: ...>` tag (NOT `<MEDIA_DOCUMENT:>`, which the
            // gateway MediaTagExtractor does not recognize — its OPEN_TAG is
            // "<MEDIA:" and the substring search never matches "<MEDIA_DOCUMENT:").
            // The extractor routes the `.mp4`/`.mov`/`.webm` extension to
            // MediaKind::Video regardless of size, and each surface dispatcher does
            // its own cap handling, so the over-cap video is recognized and routed
            // instead of being rendered to the user as a bare filesystem path with
            // no attachment (the silent-drop D-07 forbids).
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
// VideoAnimateTool — image-to-video (I2V)
// ---------------------------------------------------------------------------

/// Image-to-video tool backed by the fal.ai LTX-2.3 I2V queue API.
///
/// `image_url` accepts:
/// - A public HTTPS URL: validated by `is_safe_url` in `spawn_blocking`, then
///   passed unchanged to fal.
/// - A local filesystem path: must canonicalize under `$IRONHERMES_HOME/cache/`
///   (path traversal guard), then read as bytes and encoded as a
///   `data:<mime>;base64,<b64>` URI (D-02).
pub struct VideoAnimateTool {
    config: Arc<Config>,
    client: FalClient,
    /// Venice.ai client (Phase 47 D-01/D-02). Defaults to production
    /// `VeniceClient::new()`; overridable via [`VideoAnimateTool::with_venice_client`].
    venice_client: VeniceClient,
    session_key: Option<SessionKey>,
    counter: Option<VideoSessionCounter>,
    ack_sink: Option<Arc<dyn AckSink>>,
    /// Shared spend guardrail (Phase 47 Plan 05, GEN-05). `None` = not
    /// enforced (pre-Plan-08 call sites) — the per-session cap still applies.
    guardrail: Option<Arc<GenerationGuardrail>>,
    /// D-08: `Root` (direct chat, exempt from `per_child_cap`/the descendant
    /// pool) or `Descendant` (delegate/kanban, subject to both tiers).
    /// Defaults to `Root`; wired by Plan 08 at production construction time.
    reservation_kind: ReservationKind,
    /// Plan 08 (D-10): optional artifact-gallery capture sink, wired only for
    /// delegate children.
    capture_sink: Option<Arc<dyn ArtifactCaptureSink>>,
}

impl VideoAnimateTool {
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

    /// Per-session construction (production path).
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
    /// need a new required parameter (mirrors `ImageGenTool::with_guardrail`).
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

    /// Cap check + atomic reservation (WR-01) — same shared logic as
    /// `VideoGenerateTool::try_reserve_slot`, sharing the same counter (both tools
    /// share the session's video quota). Returns a [`SlotGuard`] holding the
    /// reservation for the lifetime of the generation.
    pub fn try_reserve_slot(&self) -> Result<SlotGuard, String> {
        reserve_video_slot(
            &self.session_key,
            &self.counter,
            self.config.video_gen.session_cap,
        )
    }

    fn emit_interim_ack(&self) {
        if let Some(sink) = &self.ack_sink {
            sink.ack("Animating your image… this may take a few minutes.");
        }
    }

    /// Resolve the `image_url` argument into a fal-safe string:
    ///
    /// - `https://` / `http://` URLs: validate with `is_safe_url` in `spawn_blocking`
    ///   (SSRF guard), then return unchanged.  When `skip_ssrf` is `true` (test-only
    ///   via `FalClient::allow_loopback()`) the SSRF check is bypassed so wiremock
    ///   servers on loopback can act as image sources without failing the guard.
    /// - Everything else treated as a local path: `canonicalize()` → assert prefix
    ///   under `$IRONHERMES_HOME/cache/` (path traversal guard) → size-gate against
    ///   `max_inline_bytes` (CR-01: bound the read BEFORE allocating) → read bytes →
    ///   detect MIME from extension → encode as `data:<mime>;base64,<b64>` URI.
    ///
    /// `max_inline_bytes` is the configurable inline cap (WR-01:
    /// `config.video_gen.max_inline_bytes`). The local file is stat'd first and
    /// rejected with a clear, non-retried error when it exceeds the cap — so an
    /// LLM-supplied path to an arbitrarily large cache file can never trigger an
    /// unbounded heap allocation (memory-exhaustion DoS) or a fal POST.
    async fn resolve_image_url(
        raw: &str,
        skip_ssrf: bool,
        max_inline_bytes: u64,
    ) -> anyhow::Result<String> {
        // Defensive guard (fix 36.3.3): a `data:` URI is inline image data, not a path
        // or a public URL. The model occasionally tries to paste the vision data URI
        // here; that produces an enormous tool-call JSON that truncates mid-string.
        // Reject it early with a clear, NON-retried message pointing at the cache path.
        if raw.starts_with("data:") {
            anyhow::bail!(
                "video_animate needs an image file path or a public URL, not inline image data. Pass the cache path of the image (e.g. an attached photo or a prior image_generate result)."
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
                        "image_url rejected by SSRF check: only publicly accessible URLs are allowed"
                    );
                }
            }
            return Ok(raw.to_string());
        }

        // Local path branch — must be under the cache root.
        // WR-03: use tokio::fs for the canonicalize/stat/read sequence so these
        // blocking syscalls (and the up-to-`max_inline_bytes` read) run on the
        // blocking pool instead of stalling the async runtime worker thread,
        // mirroring the SSRF spawn_blocking offload above.
        let path = std::path::Path::new(raw);
        let canonical = tokio::fs::canonicalize(path)
            .await
            .map_err(|e| anyhow::anyhow!("image_url path could not be resolved: {e}"))?;

        let cache_root = ironhermes_core::constants::get_hermes_home().join("cache");
        if !canonical.starts_with(&cache_root) {
            anyhow::bail!(
                "image_url path is outside the allowed cache root ({}) — path traversal rejected",
                cache_root.display()
            );
        }

        // CR-01: size-gate BEFORE reading the file into memory. The cache root
        // contains LLM-written generated media of unbounded size; an unbounded
        // `std::fs::read` + base64 (~1.33x) + format! (a third copy) of a huge
        // file is a memory-exhaustion DoS. Stat first and reject over-cap with a
        // clear, non-retried error and NO fal call (WR-01: cap is configurable
        // via `config.video_gen.max_inline_bytes`).
        let meta = tokio::fs::metadata(&canonical)
            .await
            .map_err(|e| anyhow::anyhow!("image_url path could not be stat'd: {e}"))?;
        if meta.len() > max_inline_bytes {
            anyhow::bail!(
                "image file is too large to inline ({} bytes > {} cap)",
                meta.len(),
                max_inline_bytes
            );
        }

        let bytes = tokio::fs::read(&canonical)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read image file: {e}"))?;

        let mime = mime_from_extension(canonical.extension().and_then(|e| e.to_str()));
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(format!("data:{mime};base64,{b64}"))
    }
}

/// Map file extension to MIME type for the base64 data URI.
fn mime_from_extension(ext: Option<&str>) -> &'static str {
    match ext {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        // WR-05: unknown/missing extension → application/octet-stream (the locked
        // contract's fallback), NOT image/png. Mislabeling a non-image cache file
        // as image/png produced a confusing fal-side decode error instead of a
        // clear signal that the bytes are not a recognized image type.
        _ => "application/octet-stream",
    }
}

#[async_trait]
impl Tool for VideoAnimateTool {
    fn name(&self) -> &str {
        "video_animate"
    }

    fn toolset(&self) -> &str {
        "web"
    }

    fn description(&self) -> &str {
        "Animate a still image into a short video clip and deliver it to the user as a native \
         attachment. Provide `image_url` (a public HTTPS URL or a local cache path) and an \
         optional `prompt` to guide the animation style."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "video_animate",
            "Animate a still image into a short video clip and deliver it as a native attachment.",
            json!({
                "type": "object",
                "properties": {
                    "image_url": {
                        "type": "string",
                        "description": "A publicly accessible HTTPS URL or a local cache path to \
                                        the image to animate."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Optional text prompt to guide the animation direction \
                                        (e.g. \"pan left\", \"zoom in slowly\")."
                    },
                    "model": {
                        "type": "string",
                        "description": "Optional fal model id to override the configured default \
                                        (e.g. \"fal-ai/ltx-2.3/image-to-video\")."
                    }
                },
                "required": ["image_url"]
            }),
        )
    }

    fn prerequisites(&self) -> Vec<crate::registry::Prerequisite> {
        vec![crate::registry::Prerequisite {
            kind: "env_var".to_string(),
            name: "FAL_KEY".to_string(),
            description: "fal.ai API key — required for video_animate to animate images."
                .to_string(),
            required: true,
            group: None,
        }]
    }

    /// D-04 (Phase 41.3): provider-side video generation routinely exceeds
    /// the 60s trait default.
    fn timeout_secs(&self) -> Option<u64> {
        Some(900)
    }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        let raw_image_url = args["image_url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: image_url"))?;

        // Resolve image URL (SSRF + path traversal guard + CR-01 size-gate) BEFORE
        // the cap check, so a bad input fails fast without consuming a slot.
        // skip_ssrf=true only when client has allow_loopback_cdn set (test-only path).
        // WR-01: the inline size cap is the configurable `max_inline_bytes`.
        let image_url = Self::resolve_image_url(
            raw_image_url,
            self.client.allow_loopback(),
            self.config.video_gen.max_inline_bytes,
        )
        .await?;

        // D-08: model override per call; otherwise the configured i2v default
        // (Phase 47 D-01/D-03 — per-mode, replaces the legacy flat
        // `default_i2v_model` read).
        let effective_model = args["model"]
            .as_str()
            .filter(|m| !m.is_empty())
            .map(|m| m.to_string())
            .unwrap_or_else(|| self.config.video_gen.i2v.model.clone());

        // D-06 / WR-01: atomically reserve a per-session slot. The guard holds the
        // reservation for the whole render and auto-releases on any early return
        // below; it is committed only on success.
        let slot = match self.try_reserve_slot() {
            Ok(slot) => slot,
            Err(blocked) => return Err(anyhow::anyhow!(blocked)),
        };

        // GEN-05 / T-47-04e: the shared guardrail chokepoint runs BEFORE any
        // provider call too. A `Root` reservation always succeeds here (D-08 —
        // bounded only by the session_cap check above).
        if let Some(guardrail) = &self.guardrail {
            guardrail
                .try_reserve(&self.reservation_kind, "i2v", &effective_model)
                .map_err(|block| anyhow::anyhow!(block.message()))?;
        }

        // D-02: resolve fal vs venice from the effective model + configured
        // provider — NEVER from the config default alone (Plan 04 prohibition).
        let backend = gen_backend::resolve(
            self.config.video_gen.i2v.provider.as_deref(),
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

                // Build the I2V body: same fields as T2V plus image_url.
                // WR-04: use "aspect_ratio":"auto" (not "16:9") so the output
                // ratio tracks the source image instead of letterboxing/
                // distorting portrait or square inputs. The
                // fal-ai/ltx-2.3/image-to-video model documents aspect_ratio
                // as an enum auto|16:9|9:16 with "auto" the default (preserves
                // source) — see 36.3.3-RESEARCH.md, cited from
                // fal.ai/models/fal-ai/ltx-2.3/image-to-video/api — so "auto"
                // is a valid value and matches the locked design contract.
                // (Kept exactly as shipped — the fal arm is unchanged.)
                let body = json!({
                    "image_url": image_url,
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

                // Pitfall 7: wrap timeout to say "video rendering timed out".
                self.client
                    .poll(&fal_key, &submit, timeout)
                    .await
                    .map_err(|e| anyhow::anyhow!("video rendering timed out: {e}"))?;

                let result = self.client.fetch_video(&fal_key, &submit).await?;

                // WR-01: thread the configurable download cap
                // (config.video_gen.max_inline_bytes) instead of relying on the
                // hardcoded VIDEO_SIZE_CAP const default.
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
                // means "model-determined" → omit that param. Note every Venice
                // image-to-video model returns `aspect_ratios: []` (the ratio is
                // taken from the input image), so aspect_ratio is normally
                // omitted here — the old fail-closed validator wrongly rejected
                // it, blocking i2v entirely.
                let entries = self
                    .venice_client
                    .fetch_models(&venice_key, "video")
                    .await
                    .map_err(|e| venice_video_error(e, timeout))?;
                let constraints =
                    video_constraints_for_model(entries, &effective_model, "video_gen.i2v")?;

                let cfg = &self.config.video_gen;
                let duration = VeniceClient::resolve_video_duration(
                    cfg.default_duration_secs,
                    &constraints.durations,
                    &effective_model,
                    "video_gen.i2v.duration_secs",
                )
                .map_err(|msg| anyhow::anyhow!(msg))?;
                let resolution = VeniceClient::resolve_video_param(
                    "resolution",
                    &cfg.resolution,
                    &constraints.resolutions,
                    &effective_model,
                    "video_gen.i2v.resolution",
                )
                .map_err(|msg| anyhow::anyhow!(msg))?;
                let aspect_ratio = VeniceClient::resolve_video_param(
                    "aspect_ratio",
                    &cfg.aspect_ratio,
                    &constraints.aspect_ratios,
                    &effective_model,
                    "video_gen.i2v.aspect_ratio",
                )
                .map_err(|msg| anyhow::anyhow!(msg))?;

                // D-12: resolution/aspect_ratio/duration come from config,
                // never from LLM args — params the model does not constrain are
                // omitted rather than sent blank.
                let mut body = json!({
                    "model": &effective_model,
                    "image_url": image_url,
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

                // Task 3: wire the config-gated periodic progress ping to the
                // retrieve_video on_tick hook, riding the existing AckSink
                // (Pitfall 6 — no new WS event). progress_ping_secs = 0/absent
                // is a no-op (today's single-ack behavior).
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

        // WR-01: commit the reservation on success.
        slot.commit();
        if let Some(guardrail) = &self.guardrail {
            guardrail.record_success(&self.reservation_kind, "i2v", &effective_model);
        }

        // D-09: raw usage tracing.
        debug!(
            model = %effective_model,
            latency_ms = started.elapsed().as_millis() as u64,
            request_id = %request_id,
            "video_animate completed"
        );

        // Plan 08 (D-10): best-effort artifact-gallery capture for a
        // delegate child (see `image_gen.rs`'s `ArtifactCaptureSink` doc).
        if let Some(sink) = &self.capture_sink {
            let media_path = match &outcome {
                DownloadOutcome::Video(p) | DownloadOutcome::OversizedDocument(p) => {
                    p.display().to_string()
                }
                DownloadOutcome::Photo(p) => p.display().to_string(),
            };
            sink.capture("video_animate", &media_path, raw_image_url);
        }

        match outcome {
            DownloadOutcome::Video(abs_path) => Ok(format!(
                "Generated your video.\n<MEDIA: {}>",
                abs_path.display()
            )),
            // WR-02: emit a `<MEDIA: ...>` tag (NOT `<MEDIA_DOCUMENT:>`, which the
            // gateway MediaTagExtractor does not recognize — its OPEN_TAG is
            // "<MEDIA:" and the substring search never matches "<MEDIA_DOCUMENT:").
            // The extractor routes the `.mp4`/`.mov`/`.webm` extension to
            // MediaKind::Video regardless of size, and each surface dispatcher does
            // its own cap handling, so the over-cap video is recognized and routed
            // instead of being rendered to the user as a bare filesystem path with
            // no attachment (the silent-drop D-07 forbids).
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

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn tool() -> VideoGenerateTool {
        VideoGenerateTool::new(Arc::new(Config::default()), FalClient::new())
    }

    fn animate_tool() -> VideoAnimateTool {
        VideoAnimateTool::new(Arc::new(Config::default()), FalClient::new())
    }

    #[test]
    fn t2v_name_is_video_generate() {
        assert_eq!(tool().name(), "video_generate");
    }

    #[test]
    fn i2v_name_is_video_animate() {
        assert_eq!(animate_tool().name(), "video_animate");
    }

    #[test]
    fn prerequisites_declare_fal_key() {
        for prereqs in [tool().prerequisites(), animate_tool().prerequisites()] {
            assert_eq!(prereqs.len(), 1);
            assert_eq!(prereqs[0].kind, "env_var");
            assert_eq!(prereqs[0].name, "FAL_KEY");
            assert!(prereqs[0].required);
        }
    }

    #[test]
    fn t2v_schema_exposes_prompt_and_model_not_cap() {
        let schema = tool().schema();
        let props = schema
            .function
            .parameters
            .get("properties")
            .and_then(|p| p.as_object())
            .expect("schema must have properties");
        assert!(props.contains_key("prompt"));
        assert!(props.contains_key("model"));
        assert!(!props.contains_key("session_cap"));
        assert!(!props.contains_key("timeout_secs"));
    }

    #[test]
    fn i2v_schema_exposes_image_url_and_prompt() {
        let schema = animate_tool().schema();
        let props = schema
            .function
            .parameters
            .get("properties")
            .and_then(|p| p.as_object())
            .expect("schema must have properties");
        assert!(props.contains_key("image_url"));
        assert!(props.contains_key("prompt"));
        assert!(props.contains_key("model"));
    }

    #[test]
    fn t2v_body_excludes_num_inference_steps() {
        let cfg = Config::default();
        let body = VideoGenerateTool::build_t2v_body("test", "fal-ai/ltx-2.3/text-to-video", &cfg);
        assert!(body.get("prompt").is_some(), "body must have prompt");
        assert!(body.get("duration").is_some(), "body must have duration");
        assert!(
            body.get("resolution").is_some(),
            "body must have resolution"
        );
        assert!(
            body.get("aspect_ratio").is_some(),
            "body must have aspect_ratio"
        );
        assert!(body.get("fps").is_some(), "body must have fps");
        assert!(
            body.get("num_inference_steps").is_none(),
            "body must NOT have num_inference_steps (image-only field)"
        );
    }

    #[test]
    fn mime_from_extension_returns_correct_types() {
        assert_eq!(mime_from_extension(Some("png")), "image/png");
        assert_eq!(mime_from_extension(Some("jpg")), "image/jpeg");
        assert_eq!(mime_from_extension(Some("jpeg")), "image/jpeg");
        assert_eq!(mime_from_extension(Some("gif")), "image/gif");
        assert_eq!(mime_from_extension(Some("webp")), "image/webp");
        // WR-05: unknown/missing extension falls back to application/octet-stream
        // (the locked contract), not image/png.
        assert_eq!(mime_from_extension(None), "application/octet-stream");
        assert_eq!(mime_from_extension(Some("xyz")), "application/octet-stream");
    }

    #[tokio::test]
    async fn resolve_image_url_rejects_data_uri() {
        // fix 36.3.3: inline `data:` image data must be rejected with a clear,
        // non-retried error that points the model at the cache PATH instead.
        let err = VideoAnimateTool::resolve_image_url(
            "data:image/png;base64,AAAA",
            false,
            10 * 1024 * 1024,
        )
        .await
        .expect_err("a data: URI must be rejected, not accepted");
        let msg = err.to_string();
        assert!(
            msg.contains("inline image data") && msg.contains("path"),
            "error should mention inline image data and a path, got: {msg}"
        );
    }

    // -------------------------------------------------------------------
    // Phase 47 Plan 06 Task 2 — GenBackend dispatch + D-13 + Venice errors
    // -------------------------------------------------------------------

    /// Serialize IRONHERMES_HOME env mutation across tests in this module
    /// (parallel-safe) — mirrors the established pattern in `gen_backend.rs`.
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

    /// Mount a `GET /models?type=video` catalog whose single entry matches
    /// `model` and whose constraints allow exactly `config.video_gen`'s
    /// `resolution`/`aspect_ratio`/`default_duration_secs`.
    async fn mount_matching_video_model_catalog(
        server: &wiremock::MockServer,
        model: &str,
        cfg: &Config,
    ) {
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/models"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "data": [{
                    "id": model,
                    "model_spec": {
                        "constraints": {
                            "aspect_ratios": [cfg.video_gen.aspect_ratio.clone()],
                            "resolutions": [cfg.video_gen.resolution.clone()],
                            "durations": [format!("{}s", cfg.video_gen.default_duration_secs)],
                            "model_type": "text-to-video",
                            "audio": false,
                            "audio_configurable": false,
                        }
                    }
                }]
            })))
            .mount(server)
            .await;
    }

    /// A venice t2v model validates duration/resolution/aspect against the
    /// model_spec, queues, and returns bytes on COMPLETED (mocked) — the
    /// unchanged `<MEDIA:>` video tag is emitted.
    #[tokio::test]
    async fn venice_t2v_model_dispatches_validates_and_emits_media_tag() {
        let _home = HermesHomeGuard::new();
        let server = MockServer::start().await;
        let model = "wan-2-7-text-to-video";
        let config = Config::default(); // t2v defaults to venice/wan-2-7-text-to-video

        mount_matching_video_model_catalog(&server, model, &config).await;

        Mock::given(method("POST"))
            .and(path("/video/queue"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": model,
                "queue_id": "q-t2v-1",
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(vec![0u8; 32], "video/mp4"))
            .mount(&server)
            .await;

        let tool = VideoGenerateTool::new(Arc::new(config), FalClient::new())
            .with_venice_client(VeniceClient::with_base_url(server.uri()));

        let prior = std::env::var("VENICE_API_KEY").ok();
        unsafe {
            std::env::set_var("VENICE_API_KEY", "test-venice-key-not-real");
        }
        let result = tool.execute(json!({"prompt": "a dance"})).await;
        unsafe {
            match prior {
                Some(v) => std::env::set_var("VENICE_API_KEY", v),
                None => std::env::remove_var("VENICE_API_KEY"),
            }
        }

        let output = result.expect("venice t2v dispatch must succeed");
        assert!(
            output.starts_with("Generated your video.\n<MEDIA: "),
            "must preserve the unchanged video MEDIA tag, got: {output}"
        );
        assert!(
            output.contains(".mp4"),
            "must reference an .mp4 file, got: {output}"
        );
    }

    /// D-13: a configured duration outside the model's allowed set is rejected
    /// PRE-call with the exact template — no `queue_video` call is made.
    #[tokio::test]
    async fn venice_t2v_out_of_range_duration_rejected_pre_call_no_queue_call() {
        let server = MockServer::start().await;
        let model = "wan-2-7-text-to-video";

        let mut config = Config::default();
        config.video_gen.t2v.provider = Some("venice".to_string());
        config.video_gen.t2v.model = model.to_string();
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
                            "model_type": "text-to-video",
                        }
                    }
                }]
            })))
            .mount(&server)
            .await;
        // Deliberately NO /video/queue mock — if the code called queue_video
        // anyway, the request would 404 and the test would still fail (below)
        // by asserting zero requests were made to that path.

        let tool = VideoGenerateTool::new(Arc::new(config), FalClient::new())
            .with_venice_client(VeniceClient::with_base_url(server.uri()));

        let prior = std::env::var("VENICE_API_KEY").ok();
        unsafe {
            std::env::set_var("VENICE_API_KEY", "k");
        }
        let err = tool
            .execute(json!({"prompt": "x"}))
            .await
            .expect_err("out-of-range duration must be rejected pre-call");
        unsafe {
            match prior {
                Some(v) => std::env::set_var("VENICE_API_KEY", v),
                None => std::env::remove_var("VENICE_API_KEY"),
            }
        }

        let msg = err.to_string();
        assert_eq!(
            msg,
            "duration 6s not supported by wan-2-7-text-to-video; allowed: 3s, 5s, 8s. Fix video_gen.t2v.duration_secs."
        );

        let reqs = server.received_requests().await.unwrap_or_default();
        assert!(
            reqs.iter().all(|r| r.url.path() != "/video/queue"),
            "no /video/queue call may happen on a D-13 rejection"
        );
    }

    /// A Venice job non-2xx during retrieve surfaces the non-retried
    /// "Video generation failed: {reason}." template.
    #[tokio::test]
    async fn venice_t2v_job_failure_surfaces_non_retried_failed_message() {
        let server = MockServer::start().await;
        let model = "wan-2-7-text-to-video";
        let config = Config::default();

        mount_matching_video_model_catalog(&server, model, &config).await;
        Mock::given(method("POST"))
            .and(path("/video/queue"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": model,
                "queue_id": "q-fail-1",
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(
                ResponseTemplate::new(422).set_body_json(json!({"error": "content policy"})),
            )
            .mount(&server)
            .await;

        let tool = VideoGenerateTool::new(Arc::new(config), FalClient::new())
            .with_venice_client(VeniceClient::with_base_url(server.uri()));

        let prior = std::env::var("VENICE_API_KEY").ok();
        unsafe {
            std::env::set_var("VENICE_API_KEY", "k");
        }
        let err = tool
            .execute(json!({"prompt": "x"}))
            .await
            .expect_err("a Venice job non-2xx must be a terminal error");
        unsafe {
            match prior {
                Some(v) => std::env::set_var("VENICE_API_KEY", v),
                None => std::env::remove_var("VENICE_API_KEY"),
            }
        }

        let msg = err.to_string();
        assert!(
            msg.starts_with("Video generation failed: "),
            "must use the exact non-retried failure template, got: {msg}"
        );
        assert!(msg.ends_with('.'), "must end with a period, got: {msg}");
        assert!(
            !msg.to_lowercase().contains("retry") && !msg.to_lowercase().contains("try again"),
            "must be non-retried, got: {msg}"
        );
    }

    /// A Venice poll that never completes surfaces the non-retried
    /// "Video generation timed out after {timeout}s." template.
    #[tokio::test]
    async fn venice_t2v_poll_timeout_surfaces_non_retried_timeout_message() {
        let server = MockServer::start().await;
        let model = "wan-2-7-text-to-video";
        let mut config = Config::default();
        config.video_gen.timeout_secs = 1;

        mount_matching_video_model_catalog(&server, model, &config).await;
        Mock::given(method("POST"))
            .and(path("/video/queue"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": model,
                "queue_id": "q-timeout-1",
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "PROCESSING"})))
            .mount(&server)
            .await;

        let tool = VideoGenerateTool::new(Arc::new(config), FalClient::new())
            .with_venice_client(VeniceClient::with_base_url(server.uri()));

        let prior = std::env::var("VENICE_API_KEY").ok();
        unsafe {
            std::env::set_var("VENICE_API_KEY", "k");
        }
        let err = tool
            .execute(json!({"prompt": "x"}))
            .await
            .expect_err("a never-completing job must time out");
        unsafe {
            match prior {
                Some(v) => std::env::set_var("VENICE_API_KEY", v),
                None => std::env::remove_var("VENICE_API_KEY"),
            }
        }

        let msg = err.to_string();
        assert_eq!(msg, "Video generation timed out after 1s.");
    }

    /// A `Descendant` guardrail at `session_pool = 0` blocks `execute()` for
    /// the t2v tool, and no provider call is ever made (proven by pointing
    /// the fal client at an unreachable loopback address, since the guardrail
    /// check happens before backend resolution).
    #[tokio::test]
    async fn t2v_descendant_guardrail_at_pool_zero_blocks_before_any_provider_call() {
        let guardrail = Arc::new(GenerationGuardrail::new(0, 10, "root-1"));
        let tool = VideoGenerateTool::new(
            Arc::new(Config::default()),
            FalClient::with_base_url("http://127.0.0.1:1"),
        )
        .with_guardrail(
            guardrail,
            ReservationKind::Descendant {
                child_id: "child-1".to_string(),
            },
        );

        let err = tool
            .execute(json!({"prompt": "x"}))
            .await
            .expect_err("session_pool=0 must block the Descendant reservation");
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("pool"),
            "must surface the guardrail's pool-exhausted message, got: {msg}"
        );
    }
}

// -----------------------------------------------------------------------
// Phase 47 Plan 06 Task 3 — config-gated periodic progress ping
// -----------------------------------------------------------------------

#[cfg(test)]
mod progress_ping_tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Records every `ack()` message in order — lets tests assert both the
    /// exact copy and the count of pings emitted.
    struct RecordingAckSink {
        messages: Arc<StdMutex<Vec<String>>>,
    }

    impl AckSink for RecordingAckSink {
        fn ack(&self, message: &str) {
            self.messages.lock().unwrap().push(message.to_string());
        }
    }

    /// `cadence = 0` is a permanent no-op: repeated ticks (even across real
    /// elapsed time) never ping.
    #[test]
    fn progress_pinger_cadence_zero_never_pings() {
        let messages = Arc::new(StdMutex::new(Vec::new()));
        let sink: Arc<dyn AckSink> = Arc::new(RecordingAckSink {
            messages: messages.clone(),
        });
        let pinger = ProgressPinger::new(Duration::from_secs(0), Some(sink));
        for _ in 0..5 {
            pinger.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            messages.lock().unwrap().is_empty(),
            "cadence=0 must never ping"
        );
    }

    /// `cadence > 0`: no ping before the interval elapses; exactly one ping
    /// once it has; no immediate re-ping right after.
    #[test]
    fn progress_pinger_cadence_positive_pings_after_interval_elapses() {
        let messages = Arc::new(StdMutex::new(Vec::new()));
        let sink: Arc<dyn AckSink> = Arc::new(RecordingAckSink {
            messages: messages.clone(),
        });
        let pinger = ProgressPinger::new(Duration::from_millis(20), Some(sink));

        // Immediately after construction, elapsed ~0 < 20ms — no ping yet.
        pinger.tick();
        assert!(
            messages.lock().unwrap().is_empty(),
            "must not ping before the cadence elapses"
        );

        std::thread::sleep(Duration::from_millis(30));
        pinger.tick();
        {
            let msgs = messages.lock().unwrap();
            assert_eq!(msgs.len(), 1, "must ping once the cadence has elapsed");
            assert_eq!(msgs[0], "Still working on your video…");
        }

        // Immediately again — the interval was just reset, so no re-ping yet.
        pinger.tick();
        assert_eq!(
            messages.lock().unwrap().len(),
            1,
            "must not re-ping immediately after a ping"
        );
    }

    /// Integration: a poll spanning multiple PROCESSING iterations with
    /// `progress_ping_secs = 1` emits at least one "Still working on your
    /// video…" ping via the on_tick hook (existing AckSink channel).
    #[tokio::test]
    async fn video_retrieve_with_progress_ping_cadence_positive_emits_periodic_pings() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "PROCESSING"})))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(vec![0u8; 8], "video/mp4"))
            .mount(&server)
            .await;

        let job = QueuedVideoJob {
            model: "m".to_string(),
            queue_id: "q".to_string(),
            download_url: None,
        };
        let venice_client = VeniceClient::with_base_url(server.uri());
        let messages = Arc::new(StdMutex::new(Vec::new()));
        let sink: Arc<dyn AckSink> = Arc::new(RecordingAckSink {
            messages: messages.clone(),
        });

        let bytes = retrieve_video_with_progress_ping(
            &venice_client,
            "key",
            &job,
            Duration::from_secs(5),
            1, // progress_ping_secs
            Some(sink),
        )
        .await
        .expect("retrieve must succeed");
        assert_eq!(bytes, vec![0u8; 8]);

        let msgs = messages.lock().unwrap();
        assert!(
            !msgs.is_empty(),
            "a >0 cadence over a multi-iteration poll must emit at least one ping"
        );
        assert!(
            msgs.iter().all(|m| m == "Still working on your video…"),
            "every ping must use the exact UI-SPEC copy, got: {msgs:?}"
        );
    }

    /// The same multi-iteration poll with `progress_ping_secs = 0` emits ZERO
    /// pings — only the caller's up-front interim ack (not exercised here)
    /// would ever fire; today's single-ack behavior is preserved.
    #[tokio::test]
    async fn video_retrieve_with_progress_ping_cadence_zero_emits_no_pings() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "PROCESSING"})))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/video/retrieve"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(vec![0u8; 8], "video/mp4"))
            .mount(&server)
            .await;

        let job = QueuedVideoJob {
            model: "m".to_string(),
            queue_id: "q".to_string(),
            download_url: None,
        };
        let venice_client = VeniceClient::with_base_url(server.uri());
        let messages = Arc::new(StdMutex::new(Vec::new()));
        let sink: Arc<dyn AckSink> = Arc::new(RecordingAckSink {
            messages: messages.clone(),
        });

        let bytes = retrieve_video_with_progress_ping(
            &venice_client,
            "key",
            &job,
            Duration::from_secs(5),
            0, // progress_ping_secs = off
            Some(sink),
        )
        .await
        .expect("retrieve must succeed");
        assert_eq!(bytes, vec![0u8; 8]);

        assert!(
            messages.lock().unwrap().is_empty(),
            "progress_ping_secs=0 must emit zero pings"
        );
    }

    /// No `git diff --stat` change touches `crates/iron_hermes_ui/` — the
    /// ping rides the existing AckSink, never a new WS event (Pitfall 6). This
    /// is asserted at the plan/verification level (see 47-06-PLAN.md
    /// `<verification>`); this test documents the invariant the design relies
    /// on: `AckSink` is the ONLY side channel `ProgressPinger` writes to.
    #[test]
    fn progress_pinger_only_writes_through_the_ack_sink_trait() {
        // Compile-time proof: ProgressPinger's only side-effecting field is
        // `ack_sink: Option<Arc<dyn AckSink>>` — there is no other channel
        // (e.g. no WS sender field) it could write through.
        let pinger = ProgressPinger::new(Duration::from_secs(1), None);
        pinger.tick(); // no-op with no sink; must not panic.
    }
}

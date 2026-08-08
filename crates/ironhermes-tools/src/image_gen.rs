//! Phase 01 — `image_gen` LLM tool (text → fal.ai image → `<MEDIA:>` tag).
//!
//! The thinnest real text-to-image slice: `execute()` resolves `FAL_KEY` lazily
//! (SAFE-01), drives [`FalClient`] submit→poll→fetch→download_to_cache, and
//! returns a `<MEDIA: /abs/path>` tag (DLV-02) that the gateway delivers as a
//! native Telegram photo. The tool is hidden from the LLM schema when `FAL_KEY`
//! is unset (GEN-03) via the default `is_available()` walking `prerequisites()`.
//!
//! Per D-08 only the model is LLM-overridable; `session_cap` / `timeout_secs`
//! are read from config and NOT exposed in the schema.
//!
//! Plan 03 adds the per-session generation cap (SAFE-05 / D-03), the interim
//! "generating…" acknowledgment (D-04), and per-generation usage tracing with
//! raw units only — no cost figure (D-06). The cap is the abuse/runaway guard;
//! flux/schnell is free. The tool is now registered **per session** (mirroring
//! `register_tts_tools`): each `ImageGenTool` carries the chat's [`SessionKey`]
//! and shares a [`SessionCounter`] keyed by `SessionKey`, so one session
//! reaching the cap never blocks another.
//!
//! Phase 47 Plan 06 rewires `execute()` to dispatch through [`crate::gen_backend`]
//! (fal or venice per the *effective* `image_gen.t2i.model`, D-01/D-02) and adds
//! the shared [`GenerationGuardrail`] chokepoint (Plan 05, GEN-05) BEFORE any
//! provider call. The `<MEDIA:>` emit strings and the per-session cap are
//! unchanged; the deltas are dispatch, the guardrail, and the Venice branch.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ironhermes_core::{Config, SessionKey, ToolSchema};
use serde_json::json;
use tracing::debug;

use crate::fal::{DownloadOutcome, FalClient};
use crate::gen_backend::{self, GenBackend};
use crate::gen_guardrail::{GenerationGuardrail, ReservationKind};
use crate::registry::Tool;
use crate::venice::VeniceClient;

/// Venice `/image/generate` requested output format (Venice's own default).
/// Kept local to this file since fal.rs's `PHOTO_SIZE_CAP` is private and this
/// plan's file scope is `image_gen.rs`/`video_gen.rs` only.
const VENICE_IMAGE_FORMAT: &str = "webp";

/// Photo size cap applied to the Venice image path (D-06). Mirrors fal.rs's
/// private `PHOTO_SIZE_CAP` (20 MB, the Telegram photo cap, DLV-04) — kept as
/// a local duplicate since that constant is not `pub` and this plan does not
/// touch `fal.rs`.
const VENICE_IMAGE_SIZE_CAP: u64 = 20 * 1024 * 1024;

/// Shared, `SessionKey`-keyed generation counter.
///
/// One instance is created per registry and cloned into every per-session
/// `ImageGenTool`, so the cap is scoped to a chat session (T-03-02 mitigation):
/// session A's count never leaks into session B. `u32` counts successful
/// generations; the cap is read from `config.image_gen.session_cap`.
pub type SessionCounter = Arc<Mutex<HashMap<SessionKey, u32>>>;

/// Sink for the interim "generating…" acknowledgment (D-04).
///
/// The `Tool` trait returns a single terminal `String`, so the interim ack
/// needs a side channel. The gateway/web surfaces implement this to push an
/// out-of-band "generating…" notice to the user while the fal job runs; the
/// CLI/local path leaves it `None` (the ack is best-effort, never load-bearing).
/// Implementors must be cheap and non-blocking — `ack()` is called inline on
/// the execute path just before submit.
pub trait AckSink: Send + Sync {
    /// Surface an interim acknowledgment message to the user.
    fn ack(&self, message: &str);
}

/// Sink that captures a COMPLETED generation (image or video) into the
/// artifact gallery (Phase 47 Plan 08, D-10).
///
/// A delegate child's `<MEDIA:>` return string is only ever read by the
/// PARENT agent as a tool-result summary — it is never rendered as chat
/// media (the gateway's `<MEDIA:>` extraction runs only on the top-level
/// agent's own turn output). Without this sink, a delegate child's generated
/// image/video would be silently unreachable once the child's isolated temp
/// dir/context is gone. `crate::delegate_task::DelegateGenCaptureSink` is the
/// concrete production implementation (publishes into the same per-profile
/// gallery `crate::artifact::ArtifactTool` writes to).
///
/// Best-effort by design: implementors must never propagate a capture
/// failure (e.g. the artifact store's rendered-size cap, likely for large
/// videos) back to the caller — the tool's own `<MEDIA:>` result has already
/// succeeded independent of this capture, so `capture()` returns `()` and
/// logs internally on failure (mirrors `AckSink::ack`'s fire-and-forget
/// shape).
pub trait ArtifactCaptureSink: Send + Sync {
    /// `kind` is the producing tool's name (`"image_gen"` /
    /// `"video_generate"` / `"video_animate"` / `"video_to_video"`);
    /// `media_path` is the absolute local path to the downloaded file (the
    /// same path embedded in the `<MEDIA:>` tag); `prompt` is the originating
    /// request text, used to derive a gallery title.
    fn capture(&self, kind: &str, media_path: &str, prompt: &str);
}

/// Text-to-image tool backed by the fal.ai queue API or the Venice.ai
/// synchronous image endpoint (Phase 47 D-01/D-02).
///
/// Holds an `Arc<Config>` (for `image_gen.t2i.{provider,model}` /
/// `image_gen.timeout_secs` / `image_gen.session_cap`), a [`FalClient`], and a
/// [`VeniceClient`]. Neither `FAL_KEY` nor `VENICE_API_KEY` is ever stored — each
/// is read inside `execute()` per call, and only the key for the resolved
/// backend is read (SAFE-01 / SAFE-02).
///
/// When constructed per session via [`ImageGenTool::new_with_session`], it also
/// carries the chat's [`SessionKey`], the shared [`SessionCounter`], and an
/// optional [`AckSink`]. [`ImageGenTool::with_guardrail`] additionally injects
/// the shared [`GenerationGuardrail`] (Plan 05, GEN-05) and its
/// [`ReservationKind`] — `None`/`Root` by default so existing call sites
/// (registry.rs, pre-Phase-47 tests) keep compiling unchanged until Plan 08
/// wires the guardrail into production registration.
pub struct ImageGenTool {
    config: Arc<Config>,
    client: FalClient,
    /// Venice.ai client (Phase 47 D-01/D-02). Defaults to the production
    /// `VeniceClient::new()`; overridable in tests via
    /// [`ImageGenTool::with_venice_client`] (mirrors the `client: FalClient`
    /// constructor parameter, letting a wiremock `server.uri()` stand in for
    /// `api.venice.ai`).
    venice_client: VeniceClient,
    /// The chat session this tool instance serves. `None` for the legacy
    /// stateless construction used by older call sites / unit tests; the cap is
    /// only enforced when a session key is present.
    session_key: Option<SessionKey>,
    /// Shared `SessionKey`-keyed counter. `None` for the stateless construction.
    counter: Option<SessionCounter>,
    /// Optional interim-ack sink (D-04).
    ack_sink: Option<Arc<dyn AckSink>>,
    /// Shared spend guardrail (Phase 47 Plan 05, GEN-05). `None` = no guardrail
    /// enforced (pre-Plan-08 call sites) — the per-session cap above still
    /// applies unconditionally.
    guardrail: Option<Arc<GenerationGuardrail>>,
    /// D-08: whether this instance is a top-level/direct-chat reservation
    /// (`Root`, exempt from `per_child_cap`/the descendant pool) or a
    /// delegate/kanban descendant (`Descendant`). Defaults to `Root`; wired by
    /// Plan 08 at production construction time.
    reservation_kind: ReservationKind,
    /// Plan 08 (D-10): optional artifact-gallery capture sink, wired only for
    /// delegate children (the chat surface's `<MEDIA:>` tag is already
    /// user-visible and needs no separate capture).
    capture_sink: Option<Arc<dyn ArtifactCaptureSink>>,
}

impl ImageGenTool {
    /// Construct a stateless tool with shared config and a fal queue client.
    ///
    /// No session key / counter is attached, so the per-session cap is not
    /// enforced. Retained for unit tests and any non-session call site.
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

    /// Construct a per-session tool (SAFE-05 / D-03 / D-04).
    ///
    /// `counter` is shared across every session's tool on the same registry;
    /// `session_key` scopes this instance's slot within it. `ack_sink` surfaces
    /// the interim "generating…" notice (D-04) and may be `None`.
    pub fn new_with_session(
        session_key: SessionKey,
        config: Arc<Config>,
        client: FalClient,
        counter: SessionCounter,
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
    /// [`ReservationKind`]. Wired in production by Plan 08 (chat sessions =
    /// `Root`, delegate/kanban descendants = `Descendant`); used directly by
    /// this plan's tests. Builder-style so existing constructor call sites
    /// (`new`/`new_with_session`) never need a new required parameter.
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

    /// Override the Venice client (test-only entry point — mirrors the
    /// existing `client: FalClient` constructor parameter).
    #[must_use]
    pub fn with_venice_client(mut self, client: VeniceClient) -> Self {
        self.venice_client = client;
        self
    }

    /// Inject the artifact-gallery capture sink (Plan 08, D-10) — wired only
    /// for delegate children via `build_child_registry`.
    #[must_use]
    pub fn with_capture_sink(mut self, sink: Arc<dyn ArtifactCaptureSink>) -> Self {
        self.capture_sink = Some(sink);
        self
    }

    /// Cap check (SAFE-05 / D-03): returns `Ok(())` if this session may generate,
    /// or `Err(message)` with a clear, **non-retried** block message once the
    /// session has reached `config.image_gen.session_cap`.
    ///
    /// This is a pure read of the shared counter — it makes NO provider call, so
    /// the cap is enforced strictly before any backend dispatch. It does not
    /// mutate the count; [`ImageGenTool::record_success`] increments on a
    /// completed generation. A stateless tool (no session key) always returns
    /// `Ok`.
    pub fn try_reserve_slot(&self) -> Result<(), String> {
        let (Some(key), Some(counter)) = (&self.session_key, &self.counter) else {
            return Ok(());
        };
        let cap = self.config.image_gen.session_cap;
        let used = *counter
            .lock()
            .expect("counter mutex")
            .get(key)
            .unwrap_or(&0);
        if used >= cap {
            // Clear, session-scoped, and deliberately non-retried: it must not
            // contain "retry"/"try again" so the agent does not loop on it.
            return Err(format!(
                "Per-session image generation limit reached ({cap} images for this chat session). \
                 This is a hard limit to prevent runaway generation; no further images will be \
                 generated in this session."
            ));
        }
        Ok(())
    }

    /// Record one successful generation against this session's counter.
    ///
    /// No-op for a stateless tool. Called only after a generation completes, so
    /// a blocked or failed attempt never consumes a slot.
    pub fn record_success(&self) {
        if let (Some(key), Some(counter)) = (&self.session_key, &self.counter) {
            *counter
                .lock()
                .expect("counter mutex")
                .entry(key.clone())
                .or_insert(0) += 1;
        }
    }

    /// Surface the interim "generating…" acknowledgment (D-04) if a sink is wired.
    fn emit_interim_ack(&self) {
        if let Some(sink) = &self.ack_sink {
            sink.ack("Generating your image…");
        }
    }
}

#[async_trait]
impl Tool for ImageGenTool {
    fn name(&self) -> &str {
        "image_gen"
    }

    fn toolset(&self) -> &str {
        "web"
    }

    fn description(&self) -> &str {
        "Generate an image from a text prompt and deliver it to the user as a native \
         attachment. Provide a vivid, detailed `prompt`. Optionally override the model \
         via `model`; otherwise the configured default is used."
    }

    fn schema(&self) -> ToolSchema {
        // D-08: only `prompt` (required) and `model` (optional) are exposed.
        // `session_cap` / `timeout_secs` are config-only and intentionally absent.
        ToolSchema::new(
            "image_gen",
            "Generate an image from a text prompt and deliver it as a native attachment.",
            json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "A detailed description of the image to generate."
                    },
                    "model": {
                        "type": "string",
                        "description": "Optional fal model id to override the configured default \
                                        (e.g. \"fal-ai/flux/schnell\")."
                    }
                },
                "required": ["prompt"]
            }),
        )
    }

    fn prerequisites(&self) -> Vec<crate::registry::Prerequisite> {
        // GEN-03 / SAFE-01: the default is_available() walks this list and hides
        // image_gen from the schema when FAL_KEY is unset.
        vec![crate::registry::Prerequisite {
            kind: "env_var".to_string(),
            name: "FAL_KEY".to_string(),
            description: "fal.ai API key — required for image_gen to generate images.".to_string(),
            required: true,
            group: None,
        }]
    }

    /// D-04 (Phase 41.3): image generation can exceed the 60s trait default.
    fn timeout_secs(&self) -> Option<u64> {
        Some(300)
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let prompt = args["prompt"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: prompt"))?;

        // D-08: model override per call; otherwise the configured t2i default
        // (Phase 47 D-01/D-03 — replaces the legacy flat `default_model` read).
        let effective_model = args["model"]
            .as_str()
            .filter(|m| !m.is_empty())
            .map(|m| m.to_string())
            .unwrap_or_else(|| self.config.image_gen.t2i.model.clone());

        // SAFE-05 / D-03: enforce the per-session cap BEFORE any provider call. A
        // blocked session returns the clear, non-retried message and no backend
        // client is ever invoked (T-03-01 mitigation).
        if let Err(blocked) = self.try_reserve_slot() {
            return Err(anyhow::anyhow!(blocked));
        }

        // GEN-05 / T-47-04e: the shared guardrail chokepoint runs BEFORE any
        // provider call too. A `Root` (direct chat) reservation always succeeds
        // here (D-08 — bounded only by the session_cap check above); a
        // `Descendant` reservation is checked-and-consumed against the
        // per-child + shared-pool tiers.
        if let Some(guardrail) = &self.guardrail {
            guardrail
                .try_reserve(&self.reservation_kind, "t2i", &effective_model)
                .map_err(|block| anyhow::anyhow!(block.message()))?;
        }

        // D-02: resolve fal vs venice from the effective model + configured
        // provider — NEVER from the config default alone (Plan 04 prohibition).
        let backend = gen_backend::resolve(
            self.config.image_gen.t2i.provider.as_deref(),
            &effective_model,
        )?;

        let timeout = Duration::from_secs(self.config.image_gen.timeout_secs);
        let started = std::time::Instant::now();

        // D-04: surface the interim "generating…" acknowledgment before any
        // provider call, so the user gets immediate feedback.
        self.emit_interim_ack();

        let (outcome, request_id, image_count) = match backend {
            GenBackend::Fal(_) => {
                // SAFE-01: resolve the key lazily, inside execute() — never at boot.
                let fal_key = std::env::var("FAL_KEY")
                    .map_err(|_| anyhow::anyhow!("FAL_KEY environment variable not set"))?;

                // Drive the fal queue state machine (GEN-01) — unchanged from
                // the pre-Phase-47 path. poll/fetch follow the status_url /
                // response_url fal returns in the submit response — NOT a URL
                // rebuilt from the model id, which 405s on sub-models (e.g.
                // flux/schnell). See FalClient::poll.
                let submit = self
                    .client
                    .submit(&fal_key, &effective_model, prompt)
                    .await?;
                let request_id = submit.request_id.clone();
                self.client.poll(&fal_key, &submit, timeout).await?;
                let result = self.client.fetch(&fal_key, &submit).await?;

                let image = result.images.first().ok_or_else(|| {
                    anyhow::anyhow!("fal returned no images for request {request_id}")
                })?;

                let outcome = self
                    .client
                    .download_to_cache(&image.url, &image.content_type)
                    .await?;
                (outcome, request_id, result.images.len())
            }
            GenBackend::Venice(_) => {
                // SAFE-01: resolve the key lazily, inside execute() — never at boot.
                let venice_key = std::env::var("VENICE_API_KEY")
                    .map_err(|_| anyhow::anyhow!("VENICE_API_KEY environment variable not set"))?;

                // Venice image generation is synchronous — a single POST
                // returns the finished bytes inline (no submit/poll/fetch
                // state machine, no CDN GET, no SSRF check on the output —
                // RESEARCH Pitfall 1).
                let bytes = self
                    .venice_client
                    .generate_image(
                        &venice_key,
                        &effective_model,
                        prompt,
                        VENICE_IMAGE_FORMAT,
                        None,
                        None,
                        // Venice needs an explicit step count or it returns a blob
                        // (fix(47)); safe_mode is config-controlled. Both are
                        // config-only knobs — never LLM-overridable (D-08).
                        Some(self.config.image_gen.steps),
                        self.config.image_gen.safe_mode,
                    )
                    .await?;

                let outcome = gen_backend::write_bytes_to_image_cache(
                    &bytes,
                    VENICE_IMAGE_FORMAT,
                    VENICE_IMAGE_SIZE_CAP,
                )
                .await?;

                // Venice's image endpoint is synchronous — there is no
                // fal-style request/queue id to trace. "venice-sync" is a
                // fixed label, never a fabricated identifier (D-06).
                (outcome, "venice-sync".to_string(), 1usize)
            }
        };

        // SAFE-05 / D-03: a generation completed — consume one slot for this
        // session (existing counter), plus the guardrail's own record (if
        // wired). Both are done only on success, so blocked or failed attempts
        // (which returned early above) never count.
        self.record_success();
        if let Some(guardrail) = &self.guardrail {
            guardrail.record_success(&self.reservation_kind, "t2i", &effective_model);
        }

        // D-06: emit raw units to tracing only — model id, image count, latency,
        // request_id. NEVER the prompt body, headers, key, or any cost/dollar
        // figure (the cap is the spend guard).
        debug!(
            model = %effective_model,
            image_count,
            latency_ms = started.elapsed().as_millis() as u64,
            request_id = %request_id,
            "image_gen completed"
        );

        // Plan 08 (D-10): best-effort capture into the artifact gallery — the
        // ONLY delivery path for a delegate child (its `<MEDIA:>` return text
        // is a tool-result summary the parent reads, never rendered as chat
        // media). A no-op when no sink is wired (chat/kanban surfaces).
        if let Some(sink) = &self.capture_sink {
            let media_path = match &outcome {
                DownloadOutcome::Photo(p) | DownloadOutcome::OversizedDocument(p) => {
                    p.display().to_string()
                }
                DownloadOutcome::Video(p) => p.display().to_string(),
            };
            sink.capture("image_gen", &media_path, prompt);
        }

        // DLV-04: match the size-gate outcome. A Photo is delivered as a native
        // attachment via the absolute <MEDIA:> tag (DLV-02, one space after the
        // colon, NOT in a code fence). An OversizedDocument exceeds the photo cap
        // and falls back to a document/text-link form referencing the same path,
        // so the gateway sends it as a document rather than a photo.
        match outcome {
            DownloadOutcome::Photo(abs_path) => Ok(format!(
                "Generated your image.\n<MEDIA: {}>",
                abs_path.display()
            )),
            DownloadOutcome::OversizedDocument(abs_path) => Ok(format!(
                "Generated your image, but it is too large to send as a photo. \
                 Sending it as a document instead.\n<MEDIA_DOCUMENT: {}>",
                abs_path.display()
            )),
            // Image gen never produces a Video outcome from either backend —
            // video downloads go through the video_gen tools.
            DownloadOutcome::Video(_) => {
                unreachable!("image_gen cannot produce a Video outcome from either fal or venice")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ironhermes_core::types::Platform;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Build a tool over default config + a base-URL-injectable client.
    fn tool() -> ImageGenTool {
        ImageGenTool::new(Arc::new(Config::default()), FalClient::new())
    }

    #[test]
    fn name_is_image_gen() {
        assert_eq!(tool().name(), "image_gen");
    }

    #[test]
    fn prerequisites_declare_fal_key_env_var() {
        let prereqs = tool().prerequisites();
        assert_eq!(prereqs.len(), 1, "image_gen has exactly one prerequisite");
        assert_eq!(prereqs[0].kind, "env_var");
        assert_eq!(prereqs[0].name, "FAL_KEY");
        assert!(prereqs[0].required, "FAL_KEY prereq must be required");
    }

    #[test]
    fn schema_exposes_prompt_and_model_only() {
        let schema = tool().schema();
        let props = schema
            .function
            .parameters
            .get("properties")
            .and_then(|p| p.as_object())
            .expect("schema must have properties object");
        assert!(props.contains_key("prompt"), "schema must expose prompt");
        assert!(props.contains_key("model"), "schema must expose model");
        // D-08: config-only knobs are NOT in the schema.
        assert!(
            !props.contains_key("session_cap"),
            "session_cap must NOT be in the schema (config-only)"
        );
        assert!(
            !props.contains_key("timeout_secs"),
            "timeout_secs must NOT be in the schema (config-only)"
        );
    }

    /// GEN-03 / SAFE-01: hidden from the schema when FAL_KEY is unset, visible when set.
    ///
    /// Serialized to avoid env races with other tests that read FAL_KEY.
    #[test]
    fn is_available_gated_on_fal_key() {
        // SAFETY: single-threaded test; we save/restore the prior value.
        let prior = std::env::var("FAL_KEY").ok();
        unsafe {
            std::env::remove_var("FAL_KEY");
        }
        assert!(
            !tool().is_available(),
            "image_gen must be hidden when FAL_KEY is unset"
        );
        unsafe {
            std::env::set_var("FAL_KEY", "test-key-not-real");
        }
        assert!(
            tool().is_available(),
            "image_gen must be available when FAL_KEY is set"
        );
        // Restore prior environment.
        unsafe {
            match prior {
                Some(v) => std::env::set_var("FAL_KEY", v),
                None => std::env::remove_var("FAL_KEY"),
            }
        }
    }

    /// DLV-02: a synthesized success string carries an absolute <MEDIA:> photo tag,
    /// NOT wrapped in backticks. (The live HTTP flow is covered by Plan 02 wiremock
    /// tests; here we lock the exact emit format the gateway extractor expects.)
    #[test]
    fn media_tag_emit_format_matches_extractor_contract() {
        let abs = "/Users/example/.ironhermes/cache/images/20260620T120000-abc.png";
        let emitted = format!("Generated your image.\n<MEDIA: {abs}>");
        // Matches <MEDIA: /...> with a photo extension.
        let re = regex::Regex::new(r"<MEDIA: /[^>]+\.(png|jpg|jpeg|webp|gif)>").unwrap();
        assert!(
            re.is_match(&emitted),
            "emit must contain an absolute <MEDIA:> photo tag, got: {emitted}"
        );
        assert!(
            !emitted.contains('`'),
            "the MEDIA tag must NOT be wrapped in backticks"
        );
    }

    // -------------------------------------------------------------------
    // Phase 47 Plan 06 Task 1 — GenBackend dispatch + guardrail chokepoint
    // -------------------------------------------------------------------

    /// Serialize IRONHERMES_HOME env mutation across tests in this module
    /// (parallel-safe) — mirrors the established pattern in `gen_backend.rs`.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard: sets IRONHERMES_HOME to a fresh tempdir on construct,
    /// restores the prior value on drop. Holds the env lock for its lifetime.
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

    /// A venice t2i model dispatches to `VeniceClient::generate_image` (mocked),
    /// decodes+writes the bytes via the shared cache helper, and emits the
    /// unchanged `<MEDIA:>` photo tag.
    #[tokio::test]
    async fn venice_t2i_model_dispatches_to_venice_and_emits_unchanged_media_tag() {
        let _home = HermesHomeGuard::new();
        let server = MockServer::start().await;
        let original_bytes = b"not-a-real-webp-but-bytes-are-bytes".to_vec();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&original_bytes);

        Mock::given(method("POST"))
            .and(path("/image/generate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "gen-1",
                "images": [b64],
            })))
            .mount(&server)
            .await;

        let mut config = Config::default();
        config.image_gen.t2i.provider = Some("venice".to_string());
        config.image_gen.t2i.model = "flux-2-pro".to_string();

        let tool = ImageGenTool::new(Arc::new(config), FalClient::new())
            .with_venice_client(VeniceClient::with_base_url(server.uri()));

        let prior = std::env::var("VENICE_API_KEY").ok();
        unsafe {
            std::env::set_var("VENICE_API_KEY", "test-venice-key-not-real");
        }
        let result = tool
            .execute(json!({"prompt": "a sunset over the bay"}))
            .await;
        unsafe {
            match prior {
                Some(v) => std::env::set_var("VENICE_API_KEY", v),
                None => std::env::remove_var("VENICE_API_KEY"),
            }
        }

        let output = result.expect("venice dispatch must succeed");
        assert!(
            output.starts_with("Generated your image.\n<MEDIA: "),
            "must emit the unchanged photo MEDIA tag prefix, got: {output}"
        );
        let re = regex::Regex::new(r"<MEDIA: /[^>]+\.webp>").unwrap();
        assert!(
            re.is_match(&output),
            "must emit an absolute <MEDIA:> .webp tag, got: {output}"
        );
    }

    /// A fal-ai/* model preserves the existing fal submit/poll/fetch/download
    /// path unchanged (GEN-01 back-compat).
    #[tokio::test]
    async fn fal_ai_model_preserves_existing_fal_path() {
        let server = MockServer::start().await;
        let request_id = "req-fal-preserved";
        let base = server.uri();
        let model = "fal-ai/flux/schnell";

        Mock::given(method("POST"))
            .and(path(format!("/{model}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "request_id": request_id,
                "status_url": format!("{base}/status/{request_id}"),
                "response_url": format!("{base}/result/{request_id}"),
                "cancel_url": format!("{base}/cancel/{request_id}"),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/status/{request_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "COMPLETED"})))
            .mount(&server)
            .await;
        let cdn_url = format!("{base}/cdn/out.png");
        Mock::given(method("GET"))
            .and(path(format!("/result/{request_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "images": [{
                    "url": cdn_url,
                    "width": 64,
                    "height": 64,
                    "content_type": "image/png"
                }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/cdn/out.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 64]))
            .mount(&server)
            .await;

        let mut config = Config::default();
        config.image_gen.t2i.provider = Some("fal".to_string());
        config.image_gen.t2i.model = model.to_string();

        let tool = ImageGenTool::new(
            Arc::new(config),
            FalClient::with_base_url_allow_loopback(server.uri()),
        );

        let prior = std::env::var("FAL_KEY").ok();
        unsafe {
            std::env::set_var("FAL_KEY", "fake-key");
        }
        let result = tool.execute(json!({"prompt": "a cat"})).await;
        unsafe {
            match prior {
                Some(v) => std::env::set_var("FAL_KEY", v),
                None => std::env::remove_var("FAL_KEY"),
            }
        }

        let output = result.expect("existing fal path must be preserved");
        assert!(
            output.starts_with("Generated your image.\n<MEDIA: "),
            "fal path must preserve the unchanged MEDIA emit, got: {output}"
        );

        if let Some(p) = output
            .split("<MEDIA: ")
            .nth(1)
            .and_then(|s| s.strip_suffix('>'))
        {
            let _ = std::fs::remove_file(p.trim());
        }
    }

    /// A `Descendant` guardrail at `session_pool = 0` blocks `execute()` with the
    /// non-retried breach message, and no provider call is ever made (proven by
    /// pointing the fal client at an unreachable loopback address).
    #[tokio::test]
    async fn descendant_guardrail_at_pool_zero_blocks_before_any_provider_call() {
        let guardrail = Arc::new(GenerationGuardrail::new(0, 10, "root-1"));
        let config = Arc::new(Config::default());
        let tool = ImageGenTool::new(config, FalClient::with_base_url("http://127.0.0.1:1"))
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
        assert!(
            !msg.to_lowercase().contains("retry") && !msg.to_lowercase().contains("try again"),
            "guardrail block message must be non-retried, got: {msg}"
        );
    }

    /// D-08: a `Root` (direct chat) reservation is exempt from `per_child_cap` —
    /// it can generate MORE times than the per-child cap, bounded only by the
    /// existing `image_gen.session_cap`.
    #[tokio::test]
    async fn root_reservation_exceeds_per_child_cap_bounded_only_by_session_cap() {
        let _home = HermesHomeGuard::new();
        let guardrail = Arc::new(GenerationGuardrail::new(100, 3, "root-1"));

        let server = MockServer::start().await;
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"tiny-bytes");
        Mock::given(method("POST"))
            .and(path("/image/generate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"images": [b64]})))
            .mount(&server)
            .await;

        let mut config = Config::default();
        config.image_gen.session_cap = 5;
        config.image_gen.t2i.provider = Some("venice".to_string());
        config.image_gen.t2i.model = "flux-2-pro".to_string();

        let counter: SessionCounter = Arc::new(Mutex::new(HashMap::new()));
        let session_key = SessionKey::new(Platform::Local, "root-exemption-test");
        let tool = ImageGenTool::new_with_session(
            session_key,
            Arc::new(config),
            FalClient::new(),
            counter,
            None,
        )
        .with_guardrail(guardrail, ReservationKind::Root)
        .with_venice_client(VeniceClient::with_base_url(server.uri()));

        let prior = std::env::var("VENICE_API_KEY").ok();
        unsafe {
            std::env::set_var("VENICE_API_KEY", "test-key-not-real");
        }

        // 4 generations > per_child_cap (3) — must all succeed for a Root
        // reservation (D-08), bounded only by session_cap (5).
        for i in 0..4 {
            tool.execute(json!({"prompt": format!("img {i}")}))
                .await
                .unwrap_or_else(|e| {
                    panic!("Root reservation #{i} must succeed (exempt from per_child_cap): {e}")
                });
        }

        unsafe {
            match prior {
                Some(v) => std::env::set_var("VENICE_API_KEY", v),
                None => std::env::remove_var("VENICE_API_KEY"),
            }
        }
    }

    /// `record_success` (both the session counter and the guardrail) is only
    /// called after a successful generation — a failed provider call never
    /// consumes a slot.
    #[tokio::test]
    async fn record_success_is_only_called_after_a_successful_generation() {
        let _home = HermesHomeGuard::new();
        // No mock mounted for /image/generate — the venice call 404s.
        let server = MockServer::start().await;

        let mut config = Config::default();
        config.image_gen.t2i.provider = Some("venice".to_string());
        config.image_gen.t2i.model = "flux-2-pro".to_string();

        let counter: SessionCounter = Arc::new(Mutex::new(HashMap::new()));
        let session_key = SessionKey::new(Platform::Local, "record-only-on-success");
        let tool = ImageGenTool::new_with_session(
            session_key.clone(),
            Arc::new(config),
            FalClient::new(),
            counter.clone(),
            None,
        )
        .with_venice_client(VeniceClient::with_base_url(server.uri()));

        let prior = std::env::var("VENICE_API_KEY").ok();
        unsafe {
            std::env::set_var("VENICE_API_KEY", "k");
        }
        let result = tool.execute(json!({"prompt": "x"})).await;
        unsafe {
            match prior {
                Some(v) => std::env::set_var("VENICE_API_KEY", v),
                None => std::env::remove_var("VENICE_API_KEY"),
            }
        }

        assert!(result.is_err(), "an unmocked venice endpoint must fail");
        assert_eq!(
            *counter.lock().unwrap().get(&session_key).unwrap_or(&0),
            0,
            "a failed generation must not increment the session counter"
        );
    }
}

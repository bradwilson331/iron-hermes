//! Phase 25.1 D-04 / D-06 / D-07 / D-08 / D-09: browser_vision.
//!
//! Captures a full-page screenshot via chromiumoxide, encodes to base64 data URL,
//! and routes the multimodal call through a `VisionClientHandle` — a trait that
//! `ironhermes-agent` implements via `build_role_client("vision")` (Phase 26 D-07
//! cascade) with fallback to the main provider.
//!
//! # Dependency cycle avoidance (OQ-5)
//!
//! `ironhermes-agent` depends on `ironhermes-tools`, so `ironhermes-tools` CANNOT
//! depend on `ironhermes-agent` without creating a cycle. The solution follows the
//! Phase 20 MemoryManagerHandle precedent: define a `VisionClientHandle` trait here
//! in `ironhermes-tools`, implement it in `ironhermes-agent` (plan 09), and wire the
//! `Arc<dyn VisionClientHandle>` into `BrowserVisionTool::new` at AgentLoop init time.
//!
//! This is the FIRST real consumer of Phase 26's vision-role infrastructure.
//! Closes Phase 26 SC-2 ("vision wired") retroactively.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use ironhermes_core::ToolSchema;
use ironhermes_core::config::BrowserBackend;
use ironhermes_core::provider::ProviderResolver;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::debug;

use crate::browser_session::{
    BrowserSession, configured_browser_engine_available, configured_engine_prerequisite,
};
use crate::registry::{Prerequisite, Tool};

/// Phase 25.1 D-09: default prompt when `browser_vision` is called without a prompt arg.
pub const DEFAULT_PROMPT: &str = "Describe what's visible on this page in detail, including any interactive elements, \
     text content, and visual structure.";

// =============================================================================
// VisionClientHandle — dependency-inversion trait (OQ-5 resolution)
// =============================================================================

/// Trait that abstracts the multimodal LLM call for `browser_vision`.
///
/// Implemented by `ironhermes-agent` via `build_role_client("vision")` cascade
/// (Phase 26 D-07). Defined here in `ironhermes-tools` to avoid a circular
/// dependency: `ironhermes-agent` → `ironhermes-tools` → `ironhermes-agent`.
///
/// Contract:
/// - `prompt` is the analysis prompt (D-09).
/// - `image_data_url` is `data:image/png;base64,<b64>` (D-08).
/// - Returns the LLM response text on success.
#[async_trait]
pub trait VisionClientHandle: Send + Sync {
    async fn vision_call(&self, prompt: String, image_data_url: String) -> anyhow::Result<String>;
}

// =============================================================================
// BrowserVisionTool
// =============================================================================

/// Phase 25.1 D-04 — `browser_vision` tool.
///
/// Captures a full-page PNG screenshot, base64-encodes it, then calls the
/// vision client handle which resolves the best available multimodal LLM via
/// the Phase 26 role cascade (vision role → main provider fallback).
pub struct BrowserVisionTool {
    session: Arc<Mutex<Option<BrowserSession>>>,
    /// Used by `is_available()` to check D-06 vision capability.
    resolver: Arc<ProviderResolver>,
    /// Wired at AgentLoop init time (plan 09) — implements build_role_client cascade.
    vision_client: Arc<dyn VisionClientHandle>,
    /// Phase 53 Plan 04: the registry's config, so `is_available()` asks the
    /// same backend-aware resolver the other ten browser_* tools use.
    config: Arc<ironhermes_core::config::Config>,
}

impl BrowserVisionTool {
    /// Construct the tool. Called from `register_defaults()` (plan 09) with all
    /// Arc pointers cloned from the AgentLoop's shared state.
    pub fn new(
        session: Arc<Mutex<Option<BrowserSession>>>,
        resolver: Arc<ProviderResolver>,
        vision_client: Arc<dyn VisionClientHandle>,
        config: Arc<ironhermes_core::config::Config>,
    ) -> Self {
        Self {
            session,
            resolver,
            vision_client,
            config,
        }
    }

    /// D-06 helper: vision is available when EITHER a vision role is resolvable OR
    /// the main provider's model metadata declares `supports_vision = true`.
    pub fn vision_capable(&self) -> bool {
        if self.resolver.resolve_role("vision").is_some() {
            return true;
        }
        let main = self.resolver.resolve_for_main();
        main.model_metadata
            .as_ref()
            .map(|m| m.capabilities.vision)
            .unwrap_or(false)
    }
}

#[async_trait]
impl Tool for BrowserVisionTool {
    fn name(&self) -> &str {
        "browser_vision"
    }

    fn toolset(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Capture a screenshot of the current browser page — full-page on Chromium, \
         viewport-only on Obscura — and analyze it via the auxiliary vision role (or the \
         main provider if multimodal-capable). The tool's own JSON output names which \
         capture it performed via a `capture` field valued `full_page` or `viewport`. \
         Optional `prompt` argument narrows the analysis (e.g. 'What is the price of \
         the highlighted item?'). Default prompt describes the page contents in detail."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "browser_vision",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Optional analysis prompt. Defaults to a general 'describe this page' query."
                    }
                },
                "required": []
            }),
        )
    }

    /// D-06: available iff a browser engine is discoverable for the
    /// configured backend AND the resolver exposes a vision role or a
    /// multimodal-capable main provider.
    ///
    /// Phase 53 Plan 04: `browser_vision` is deliberately excluded from the
    /// ten other browser_* tools' mechanical rewrite. Its availability has a
    /// SECOND gate — `vision_capable()` — that the other ten don't carry.
    /// Dropping this conjunct would advertise `browser_vision` on a host
    /// with no vision-capable model; keep it.
    fn is_available(&self) -> bool {
        configured_browser_engine_available(&self.config.browser) && self.vision_capable()
    }

    /// D-06: two prerequisites — the configured engine's binary AND
    /// vision-or-multimodal-main. Only the first entry moved to the shared
    /// resolver (Phase 53 Plan 04); the second (`config_field` vision) is
    /// untouched — its existing test asserts on this entry's exact name.
    fn prerequisites(&self) -> Vec<Prerequisite> {
        vec![
            configured_engine_prerequisite(&self.config.browser),
            Prerequisite {
                kind: "config_field".to_string(),
                name: "auxiliary.vision OR multimodal-capable main provider".to_string(),
                description: "Either set auxiliary.vision: { provider, model } in config.yaml \
                               or use a main provider with supports_vision=true \
                               (e.g. gpt-4o, claude-3.5-sonnet, gemini-pro)"
                    .to_string(),
                required: true,
                group: None,
            },
        ]
    }

    /// Execute: screenshot → base64 data URL → VisionClientHandle → LLM text.
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_PROMPT)
            .to_string();

        debug!(prompt_len = prompt.len(), "browser_vision: invoked");

        // 1. Capture screenshot via chromiumoxide. Chromium can paint the whole
        //    document (`full_page(true)`); Obscura is scoped to viewport-only,
        //    because `Page.getLayoutMetrics` answers `clientWidth`/`clientHeight`
        //    as floats while `chromiumoxide_cdp` 0.9.1 types `LayoutViewport` as
        //    integers, so chromiumoxide rejects the reply before painting
        //    anything (ADR-0005 item 7). Revisit this concession when Task 2's
        //    pinned tripwire test
        //    (`get_layout_metrics_still_returns_floats_upstream_727cc46`) starts
        //    failing — that is the signal the upstream fix landed. D-07/D-08:
        //    which capture actually ran is disclosed both in the envelope's
        //    `capture` field below and, on the Obscura path, as a prefix
        //    sentence in `analysis`.
        let (screenshot_bytes, capture): (Vec<u8>, &'static str) = {
            let mut guard = self.session.lock().await;
            let sess = ensure_session(&mut guard).await?;

            use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
            use chromiumoxide::page::ScreenshotParams;

            let full_page = wants_full_page(sess.backend());
            let mut params = ScreenshotParams::builder().format(CaptureScreenshotFormat::Png);
            if full_page {
                params = params.full_page(true);
            }

            let bytes = sess
                .page
                .screenshot(params.build())
                .await
                .map_err(|e| anyhow::anyhow!("screenshot failed: {e}"))?;
            (bytes, capture_kind(full_page))
            // Guard drops here — release the session lock BEFORE the LLM round-trip
            // so other browser_* tools are not blocked during the network call.
        };

        debug!(
            bytes = screenshot_bytes.len(),
            capture, "browser_vision: screenshot captured"
        );

        // 2. Base64 encode + assemble data URL (D-08).
        let b64 = base64::engine::general_purpose::STANDARD.encode(&screenshot_bytes);
        let data_url = format!("data:image/png;base64,{}", b64);

        // 3. Route call through VisionClientHandle — implements D-07 cascade:
        //    vision role → main provider fallback (wired in ironhermes-agent plan 09).
        let analysis = self
            .vision_client
            .vision_call(prompt.clone(), data_url)
            .await
            .map_err(|e| anyhow::anyhow!("vision LLM call failed: {e}"))?;

        // 4. D-07: on the Obscura (viewport-only) path, prepend a one-sentence
        //    caveat to the prose the calling model actually reads. The model
        //    never sees the screenshot itself — `screenshot_bytes` above is a
        //    LENGTH, not the image — so this prefix, not just the `capture`
        //    envelope key, is what prevents a viewport-only crop from reading
        //    as a confident false negative (53-CONTEXT.md D-07).
        let analysis = disclose_viewport_capture(analysis, capture);

        // 5. Return structured envelope. `capture` is ALWAYS present (D-08),
        //    computed from the branch actually taken above, never from the
        //    configured backend alone.
        Ok(build_vision_envelope(
            &prompt,
            screenshot_bytes.len(),
            capture,
            analysis,
        ))
    }
}

// =============================================================================
// Pure decision functions (Phase 53 Plan 05, D-07/D-08) — factored out of
// execute() so the per-backend capture decision, the envelope shape, and the
// analysis-prefix disclosure are each unit-testable without a live CDP
// session, following this crate's established pattern (e.g.
// `browser_session.rs`'s `teardown_action`/`render_probe_outcome`).
// =============================================================================

/// D-07/D-08: does the configured backend support a full-page capture?
/// Chromium: yes, unchanged. Obscura: no — see the comment above the call
/// site in `execute()` for why. The `capture` envelope value and the
/// `analysis` prefix are both derived from THIS decision, not re-derived
/// from `backend` independently, so they can never disagree about what
/// actually ran.
fn wants_full_page(backend: BrowserBackend) -> bool {
    matches!(backend, BrowserBackend::Chromium)
}

/// D-08: the machine-readable capture-kind value, derived from the same
/// `full_page` bool `wants_full_page` returned — always one of these two
/// values, never absent.
fn capture_kind(full_page: bool) -> &'static str {
    if full_page { "full_page" } else { "viewport" }
}

/// D-07: prepend a one-sentence viewport caveat to `analysis` when the
/// capture that produced it was viewport-only (Obscura), so the caveat lands
/// in the prose the calling model actually reads rather than only in a
/// sibling JSON key it could skim past. The Chromium (`full_page`) path
/// returns `analysis` unmodified.
fn disclose_viewport_capture(analysis: String, capture: &str) -> String {
    if capture == "viewport" {
        format!(
            "Note: this screenshot shows only the visible viewport, not the full page — \
             content below the fold was not captured, so a negative answer here may mean \
             the content is off-screen rather than absent. {analysis}"
        )
    } else {
        analysis
    }
}

/// D-08: assembles the tool's JSON envelope. Pulled out as its own function
/// so a test can assert the `capture` key is present unconditionally on
/// both branches, not only when Obscura is configured.
fn build_vision_envelope(
    prompt: &str,
    screenshot_len: usize,
    capture: &'static str,
    analysis: String,
) -> String {
    json!({
        "prompt": prompt,
        "screenshot_bytes": screenshot_len,
        "capture": capture,
        "analysis": analysis
    })
    .to_string()
}

/// Ensure a BrowserSession exists in the Option, spawning one if needed.
async fn ensure_session<'a>(
    guard: &'a mut tokio::sync::MutexGuard<'_, Option<BrowserSession>>,
) -> anyhow::Result<&'a mut BrowserSession> {
    if guard.is_none() {
        let cfg = ironhermes_core::config::Config::load()
            .unwrap_or_default()
            .browser;
        let new_sess = BrowserSession::spawn(&cfg).await?;
        **guard = Some(new_sess);
    }
    Ok(guard.as_mut().expect("just inserted"))
}

// =============================================================================
// NoOpVisionHandle — public stub for registry wiring and tests
// =============================================================================

/// A no-op `VisionClientHandle` implementation used by `register_browser_tools`
/// when no real agent-side vision client is wired (e.g. in unit tests or when
/// the browser toolset is registered without plan-09 AgentLoop wiring).
///
/// The real implementation (`AnyClientVisionHandle`) lives in `ironhermes-agent`
/// and is injected via `register_browser_tools_with_vision` (plan 09).
pub struct NoOpVisionHandle;

#[async_trait]
impl VisionClientHandle for NoOpVisionHandle {
    async fn vision_call(
        &self,
        _prompt: String,
        _image_data_url: String,
    ) -> anyhow::Result<String> {
        anyhow::bail!(
            "browser_vision: no vision client wired — call register_browser_tools_with_vision instead of register_browser_tools"
        )
    }
}

// =============================================================================
// Unit tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::{config::Config, provider::ProviderResolver};
    use std::sync::OnceLock;

    // ---------------------------------------------------------------------------
    // env_lock: serialise tests that mutate environment variables — this
    // codebase's `cargo test -- --test-threads=1` path runs every test
    // sequentially in ONE process, so env var mutations leak across tests
    // without this (Phase 53 Plan 04).
    // ---------------------------------------------------------------------------

    fn env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    fn dummy_session() -> Arc<Mutex<Option<BrowserSession>>> {
        Arc::new(Mutex::new(None))
    }

    fn dummy_resolver() -> Arc<ProviderResolver> {
        let config = Config::default();
        Arc::new(ProviderResolver::build(&config).expect("default config builds resolver"))
    }

    /// A resolver whose `resolve_role("vision")` resolves — `vision_capable()`
    /// short-circuits true on this alone, regardless of the main provider's
    /// real vision metadata (Phase 53 Plan 04: deterministic across backends).
    fn vision_capable_resolver() -> Arc<ProviderResolver> {
        let mut config = Config::default();
        config.auxiliary = ironhermes_core::config::AuxiliaryConfig {
            provider: config.model.provider.clone(),
            model: String::new(),
        };
        Arc::new(ProviderResolver::build(&config).expect("aux-configured config builds resolver"))
    }

    /// A resolver whose `vision_capable()` is deterministically false: no
    /// per-task role, no auxiliary block, and a model name the static
    /// registry cannot look up metadata for (so `model_metadata` is `None`
    /// and the `unwrap_or(false)` fallback applies) — independent of
    /// whether the real default model happens to declare vision support.
    fn vision_incapable_resolver() -> Arc<ProviderResolver> {
        let mut config = Config::default();
        config.model.default = "definitely-not-a-real-model-xyz".to_string();
        Arc::new(ProviderResolver::build(&config).expect("config builds resolver"))
    }

    /// Minimal VisionClientHandle impl for structural tests (no real LLM calls).
    struct NoOpVisionClient;

    #[async_trait]
    impl VisionClientHandle for NoOpVisionClient {
        async fn vision_call(
            &self,
            _prompt: String,
            _image_data_url: String,
        ) -> anyhow::Result<String> {
            Ok("(test stub)".to_string())
        }
    }

    fn dummy_vision_client() -> Arc<dyn VisionClientHandle> {
        Arc::new(NoOpVisionClient)
    }

    fn dummy_config() -> Arc<ironhermes_core::config::Config> {
        Arc::new(Config::default())
    }

    fn make_tool() -> BrowserVisionTool {
        BrowserVisionTool::new(
            dummy_session(),
            dummy_resolver(),
            dummy_vision_client(),
            dummy_config(),
        )
    }

    // ---------------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------------

    #[test]
    fn name_and_toolset_match_d04() {
        let t = make_tool();
        assert_eq!(t.name(), "browser_vision");
        assert_eq!(t.toolset(), "browser");
    }

    /// Phase 53 Plan 04: entry 0 tracks the configured engine (Chromium by
    /// default here; entry 0 names obscura under backend: obscura, asserted
    /// separately below). Entry 1 (config_field vision) is untouched by this
    /// plan — asserted byte-identical to its pre-53 name.
    #[test]
    fn prerequisites_declare_the_configured_engine_and_vision_role() {
        let t = make_tool();
        let prereqs = t.prerequisites();
        assert_eq!(
            prereqs.len(),
            2,
            "browser_vision MUST declare BOTH the configured engine's binary AND \
             vision-or-multimodal-main"
        );
        assert!(
            prereqs
                .iter()
                .any(|p| p.kind == "binary_present" && p.name == "chromium-or-chrome"),
            "missing binary_present/chromium-or-chrome prereq for the default \
             (Chromium) backend"
        );
        assert!(
            prereqs
                .iter()
                .any(|p| p.kind == "config_field" && p.name.contains("vision")),
            "missing config_field/vision prereq"
        );

        // Entry 0 must track the configured backend — Obscura names obscura,
        // not chromium.
        let obscura_config = Arc::new(Config {
            browser: ironhermes_core::config::BrowserConfig {
                backend: ironhermes_core::config::BrowserBackend::Obscura,
                ..Default::default()
            },
            ..Default::default()
        });
        let obscura_tool = BrowserVisionTool::new(
            dummy_session(),
            dummy_resolver(),
            dummy_vision_client(),
            obscura_config,
        );
        let obscura_prereqs = obscura_tool.prerequisites();
        assert_eq!(obscura_prereqs.len(), 2);
        assert!(
            obscura_prereqs
                .iter()
                .any(|p| p.kind == "binary_present" && p.name.contains("obscura")),
            "entry 0 must name obscura when backend: obscura is configured"
        );
        assert!(
            obscura_prereqs
                .iter()
                .any(|p| p.kind == "config_field"
                    && p.name == "auxiliary.vision OR multimodal-capable main provider"),
            "entry 1 (config_field vision) must remain byte-identical regardless of backend"
        );
    }

    /// The engine resolver reports available AND vision_capable() is true —
    /// the conjunction's happy path.
    #[test]
    fn vision_is_available_when_the_engine_resolves_and_a_vision_model_exists() {
        let config = Arc::new(Config {
            browser: ironhermes_core::config::BrowserConfig {
                backend: ironhermes_core::config::BrowserBackend::Obscura,
                obscura_path: Some("/bin/sh".to_string()),
                ..Default::default()
            },
            ..Default::default()
        });
        let t = BrowserVisionTool::new(
            dummy_session(),
            vision_capable_resolver(),
            dummy_vision_client(),
            config,
        );
        assert!(
            t.is_available(),
            "engine resolves AND vision_capable() true → available"
        );
    }

    /// THIS is the regression a blind eleven-file find-and-replace would
    /// have introduced: on an Obscura-only host with a resolvable engine but
    /// no vision-capable model, browser_vision must stay unavailable. Task 3
    /// deliberately excluded this file from Task 2's mechanical rewrite
    /// specifically to keep this conjunct — a test named after the failure
    /// it exists to prevent.
    #[test]
    fn vision_is_unavailable_on_an_obscura_only_host_with_no_vision_model() {
        let config = Arc::new(Config {
            browser: ironhermes_core::config::BrowserConfig {
                backend: ironhermes_core::config::BrowserBackend::Obscura,
                obscura_path: Some("/bin/sh".to_string()),
                ..Default::default()
            },
            ..Default::default()
        });
        let t = BrowserVisionTool::new(
            dummy_session(),
            vision_incapable_resolver(),
            dummy_vision_client(),
            config,
        );
        assert!(
            !t.is_available(),
            "engine resolves but vision_capable() is false → must stay unavailable"
        );
    }

    /// The other half of the conjunction: a vision-capable resolver does not
    /// rescue an unresolvable engine.
    #[test]
    fn vision_is_unavailable_when_the_engine_does_not_resolve_even_with_a_vision_model() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: env_lock + --test-threads=1 ensure single mutator.
        unsafe {
            std::env::set_var("IRONHERMES_BROWSER_TEST_DISABLE", "1");
        }
        let config = dummy_config(); // backend: Chromium (default)
        let t = BrowserVisionTool::new(
            dummy_session(),
            vision_capable_resolver(),
            dummy_vision_client(),
            config,
        );
        let available = t.is_available();
        unsafe {
            std::env::remove_var("IRONHERMES_BROWSER_TEST_DISABLE");
        }
        assert!(
            !available,
            "vision-capable resolver but no engine discoverable → must stay unavailable"
        );
    }

    #[test]
    fn schema_prompt_is_optional() {
        let t = make_tool();
        let schema = t.schema();
        // `prompt` must NOT appear in the required array (D-09 — optional arg).
        let required = schema
            .function
            .parameters
            .get("required")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert!(
            !required.iter().any(|v| v.as_str() == Some("prompt")),
            "prompt must be optional — not in required array"
        );
        // Default prompt constant must mention "Describe" per D-09.
        assert!(
            DEFAULT_PROMPT.contains("Describe"),
            "DEFAULT_PROMPT must contain 'Describe'"
        );
    }

    #[test]
    fn vision_capable_with_default_resolver_reflects_main_provider_metadata() {
        // With a default config, supports_vision depends on metadata.
        // The test documents the contract rather than asserting a specific bool —
        // either value is acceptable; the point is no panic.
        let t = make_tool();
        let _ = t.vision_capable();
    }

    // ---------------------------------------------------------------------------
    // Phase 53 Plan 05 (D-07/D-08): per-backend capture disclosure
    // ---------------------------------------------------------------------------

    #[test]
    fn vision_requests_full_page_on_chromium() {
        assert!(
            wants_full_page(BrowserBackend::Chromium),
            "Chromium must keep requesting the full-page capture"
        );
    }

    #[test]
    fn vision_requests_viewport_only_on_obscura() {
        assert!(
            !wants_full_page(BrowserBackend::Obscura),
            "Obscura must not request the full-page capture — Page.getLayoutMetrics rejects \
             the reply before painting (ADR-0005 item 7)"
        );
    }

    #[test]
    fn the_vision_envelope_always_names_the_capture_it_performed() {
        for (full_page, expected) in [(true, "full_page"), (false, "viewport")] {
            let kind = capture_kind(full_page);
            assert_eq!(kind, expected);

            let envelope = build_vision_envelope("prompt text", 42, kind, "analysis".to_string());
            let parsed: serde_json::Value =
                serde_json::from_str(&envelope).expect("envelope must be valid JSON");
            assert_eq!(
                parsed.get("capture").and_then(|v| v.as_str()),
                Some(expected),
                "capture key must be present and correct on BOTH branches, not only \
                 when Obscura is configured (D-08)"
            );
        }
    }

    #[test]
    fn the_obscura_analysis_is_prefixed_with_the_viewport_caveat() {
        let original = "No checkout button visible.".to_string();

        let obscura_result = disclose_viewport_capture(original.clone(), "viewport");
        assert_ne!(
            obscura_result, original,
            "the viewport path must prepend a caveat sentence"
        );
        assert!(
            obscura_result.contains(&original),
            "the original analysis text must still be present verbatim"
        );
        assert!(
            obscura_result.to_lowercase().contains("viewport"),
            "the caveat must actually mention the viewport limitation"
        );

        let chromium_result = disclose_viewport_capture(original.clone(), "full_page");
        assert_eq!(
            chromium_result, original,
            "the full_page (Chromium) path must leave analysis untouched"
        );
    }

    /// Structural: the screenshot capture, the session-lock release (the
    /// `Guard drops here` marker the plan's own verify gate anchors on), and
    /// the vision LLM call must stay in that order inside `execute()` — the
    /// lock must never be held across the network round-trip. Needle strings
    /// are built from non-contiguous parts so this test's own source does not
    /// self-match ahead of the real `execute()` definition it inspects
    /// (Phase 53-01/53-02 self-referential-test pattern).
    #[test]
    fn the_session_lock_is_still_released_before_the_vision_call() {
        let source = include_str!("browser_vision.rs");
        let fn_needle = format!("{}{}", "async fn ", "execute(&self, args: serde_json::Value)");
        let fn_start = source
            .find(&fn_needle)
            .expect("execute() definition not found");
        let body = &source[fn_start..];
        let fn_end = body.find("\n    }\n}").unwrap_or(body.len());
        let body = &body[..fn_end];

        let shot_needle = format!("{}{}", "screenshot", "(");
        let drop_needle = format!("{} {}", "Guard", "drops here");
        let call_needle = format!("{}{}", "vision_", "call");

        let i_shot = body
            .find(&shot_needle)
            .expect("screenshot(...) call not found in execute()");
        let i_drop = body
            .find(&drop_needle)
            .expect("'Guard drops here' marker not found in execute()");
        let i_call = body
            .find(&call_needle)
            .expect("vision_call not found in execute()");

        assert!(i_shot < i_drop, "screenshot must happen before the guard drops");
        assert!(
            i_drop < i_call,
            "the session lock guard must drop before the vision LLM round-trip"
        );
    }
}

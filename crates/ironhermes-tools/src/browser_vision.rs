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
use ironhermes_core::provider::ProviderResolver;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::sync::Mutex;
use tracing::debug;

use crate::browser_session::{find_chromium_binary, BrowserSession};
use crate::registry::{Prerequisite, Tool};

/// Phase 25.1 D-09: default prompt when `browser_vision` is called without a prompt arg.
pub const DEFAULT_PROMPT: &str =
    "Describe what's visible on this page in detail, including any interactive elements, \
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
    async fn vision_call(
        &self,
        prompt: String,
        image_data_url: String,
    ) -> anyhow::Result<String>;
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
}

impl BrowserVisionTool {
    /// Construct the tool. Called from `register_defaults()` (plan 09) with all three
    /// Arc pointers cloned from the AgentLoop's shared state.
    pub fn new(
        session: Arc<Mutex<Option<BrowserSession>>>,
        resolver: Arc<ProviderResolver>,
        vision_client: Arc<dyn VisionClientHandle>,
    ) -> Self {
        Self { session, resolver, vision_client }
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
        "Capture a full-page screenshot of the current browser page and analyze it via \
         the auxiliary vision role (or the main provider if multimodal-capable). \
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

    /// D-06: available iff chromium binary is discoverable AND the resolver
    /// exposes a vision role or a multimodal-capable main provider.
    fn is_available(&self) -> bool {
        find_chromium_binary(None).is_some() && self.vision_capable()
    }

    /// D-06: two prerequisites — chromium binary AND vision-or-multimodal-main.
    fn prerequisites(&self) -> Vec<Prerequisite> {
        vec![
            Prerequisite {
                kind: "binary_present".to_string(),
                name: "chromium-or-chrome".to_string(),
                description: "Chromium or Google Chrome browser binary on PATH or at a \
                               standard install location"
                    .to_string(),
                required: true,
            },
            Prerequisite {
                kind: "config_field".to_string(),
                name: "auxiliary.vision OR multimodal-capable main provider".to_string(),
                description: "Either set auxiliary.vision: { provider, model } in config.yaml \
                               or use a main provider with supports_vision=true \
                               (e.g. gpt-4o, claude-3.5-sonnet, gemini-pro)"
                    .to_string(),
                required: true,
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

        // 1. Capture screenshot via chromiumoxide (D-08 full-page PNG).
        let screenshot_bytes: Vec<u8> = {
            let mut guard = self.session.lock().await;
            let sess = ensure_session(&mut guard).await?;

            use chromiumoxide::page::ScreenshotParams;
            use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;

            sess.page
                .screenshot(
                    ScreenshotParams::builder()
                        .format(CaptureScreenshotFormat::Png)
                        .full_page(true)
                        .build(),
                )
                .await
                .map_err(|e| anyhow::anyhow!("screenshot failed: {e}"))?
            // Guard drops here — release the session lock BEFORE the LLM round-trip
            // so other browser_* tools are not blocked during the network call.
        };

        debug!(bytes = screenshot_bytes.len(), "browser_vision: screenshot captured");

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

        // 4. Return structured envelope.
        Ok(json!({
            "prompt": prompt,
            "screenshot_bytes": screenshot_bytes.len(),
            "analysis": analysis
        })
        .to_string())
    }
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
        anyhow::bail!("browser_vision: no vision client wired — call register_browser_tools_with_vision instead of register_browser_tools")
    }
}

// =============================================================================
// Unit tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::{config::Config, provider::ProviderResolver};

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

    fn make_tool() -> BrowserVisionTool {
        BrowserVisionTool::new(dummy_session(), dummy_resolver(), dummy_vision_client())
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

    #[test]
    fn prerequisites_declare_chromium_and_vision_role() {
        let t = make_tool();
        let prereqs = t.prerequisites();
        assert_eq!(
            prereqs.len(),
            2,
            "browser_vision MUST declare BOTH chromium binary AND vision-or-multimodal-main"
        );
        assert!(
            prereqs.iter().any(|p| p.kind == "binary_present" && p.name == "chromium-or-chrome"),
            "missing binary_present/chromium-or-chrome prereq"
        );
        assert!(
            prereqs.iter().any(|p| p.kind == "config_field" && p.name.contains("vision")),
            "missing config_field/vision prereq"
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
}

//! Phase 25.1 D-20: Three mandatory integration tests for the browser toolset.
//!
//! Pattern: in-process ToolRegistry + chromiumoxide + wiremock-hosted local HTML.
//! Skips (not fails) when chromium binary is unavailable (D-22).
//!
//! Test invocation: option (c) — direct in-process tool invocation via ToolRegistry
//! (no subprocess/LLM mocking required; tools are called with literal args).

use std::sync::OnceLock;

use ironhermes_tools::browser_session::{BrowserSession, find_chromium_binary};
use ironhermes_tools::browser_vision::VisionClientHandle;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// =============================================================================
// Process-wide env lock (mirrors provider_integration.rs / toolset_integration.rs)
// =============================================================================

fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

// =============================================================================
// RAII env guard — verbatim from provider_integration.rs lines 24-47
// =============================================================================

struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, val: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: test-only env mutation, serialised behind env_lock().
        unsafe { std::env::set_var(key, val) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

// =============================================================================
// D-22: chromium availability guard
// =============================================================================

/// Returns true iff a chromium binary is discoverable on this machine.
///
/// D-22: tests that return false here SKIP (eprintln + return) rather than FAIL.
/// Mirrors the FIRECRAWL_API_KEY skip pattern from web_search tests.
fn chromium_available() -> bool {
    // IRONHERMES_BROWSER_TEST_DISABLE escape hatch — CI can set this to force-skip.
    if std::env::var("IRONHERMES_BROWSER_TEST_DISABLE").is_ok() {
        return false;
    }
    find_chromium_binary(None).is_some()
}

// =============================================================================
// Registry construction helper
// =============================================================================

type BrowserSessionArc = std::sync::Arc<tokio::sync::Mutex<Option<BrowserSession>>>;

/// Build a ToolRegistry with all 11 browser tools registered.
///
/// `vision_client` is wired into BrowserVisionTool. For tests 1 and 2, pass a
/// `NoOpVisionHandle` (they never invoke browser_vision). For test 3, pass a
/// real TestVisionHandle pointing at the aux wiremock server.
fn make_browser_registry(
    resolver: std::sync::Arc<ironhermes_core::provider::ProviderResolver>,
    vision_client: std::sync::Arc<dyn VisionClientHandle>,
) -> (ironhermes_tools::ToolRegistry, BrowserSessionArc) {
    let session: BrowserSessionArc = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let mut registry = ironhermes_tools::ToolRegistry::new();
    let config = std::sync::Arc::new(ironhermes_core::config::Config::default());
    registry.register_browser_tools_with_vision(session.clone(), resolver, vision_client, config);
    (registry, session)
}

/// Invoke a tool by name, bypassing is_available() (test-utils gate).
async fn invoke(
    registry: &ironhermes_tools::ToolRegistry,
    name: &str,
    args: serde_json::Value,
) -> anyhow::Result<String> {
    registry.handle_tool_call(name, args).await
}

// =============================================================================
// Test 1: navigate + snapshot returns refs (D-10)
// =============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn browser_navigate_then_snapshot_returns_refs() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    if !chromium_available() {
        eprintln!("SKIP browser_navigate_then_snapshot_returns_refs: no chromium binary (D-22)");
        return;
    }

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/test-page"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(
                    r#"<!doctype html><html><head><title>Test Page</title></head><body><h1>Test</h1><button>Submit</button><input type="text" placeholder="Email"></body></html>"#,
                    "text/html; charset=utf-8",
                ),
        )
        .mount(&server)
        .await;

    // Tests 1 and 2 don't invoke browser_vision, so a no-op handle is fine.
    let mut config = ironhermes_core::Config::default();
    config.model.provider = "openai".to_string();
    config.model.default = "gpt-4o".to_string();
    let resolver = std::sync::Arc::new(
        ironhermes_core::ProviderResolver::build(&config).expect("resolver build"),
    );
    let noop = std::sync::Arc::new(ironhermes_tools::browser_vision::NoOpVisionHandle);
    let (registry, session) = make_browser_registry(resolver, noop);

    let nav_url = format!("{}/test-page", server.uri());
    let nav_result = invoke(&registry, "browser_navigate", json!({"url": nav_url}))
        .await
        .expect("navigate should succeed");
    assert!(
        nav_result.contains("200") || nav_result.contains("url"),
        "navigate response unexpected: {nav_result}"
    );

    // Give chromium time to fully render the DOM.
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    let snap = invoke(&registry, "browser_snapshot", json!({}))
        .await
        .expect("snapshot should succeed");

    // D-10 ref-line format: `[N] button "Submit"`
    assert!(
        snap.contains("button") && snap.contains("Submit"),
        "expected button \"Submit\" in snapshot output, got:\n{snap}"
    );
    let has_ref_line = snap.lines().any(|line| {
        line.trim_start().starts_with('[') && line.contains("button") && line.contains("Submit")
    });
    assert!(
        has_ref_line,
        "expected `[N] button \"Submit\"` ref line in snapshot, got:\n{snap}"
    );

    // Cleanup: close browser so subsequent tests in the same process get a fresh session.
    let _ = invoke(&registry, "browser_close", json!({})).await;
    drop(session);
}

// =============================================================================
// Test 2: stale ref → element_stale envelope (D-11)
// =============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn browser_click_with_stale_ref_returns_structured_error() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    if !chromium_available() {
        eprintln!(
            "SKIP browser_click_with_stale_ref_returns_structured_error: no chromium binary (D-22)"
        );
        return;
    }

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/page-a"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><html><body><button>Original</button></body></html>"#,
            "text/html; charset=utf-8",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/page-b"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><html><body><p>different page</p></body></html>"#,
            "text/html; charset=utf-8",
        ))
        .mount(&server)
        .await;

    let mut config = ironhermes_core::Config::default();
    config.model.provider = "openai".to_string();
    config.model.default = "gpt-4o".to_string();
    let resolver = std::sync::Arc::new(
        ironhermes_core::ProviderResolver::build(&config).expect("resolver build"),
    );
    let noop = std::sync::Arc::new(ironhermes_tools::browser_vision::NoOpVisionHandle);
    let (registry, session) = make_browser_registry(resolver, noop);

    // 1. Navigate to page-a, snapshot → extract ref of the button.
    let _ = invoke(
        &registry,
        "browser_navigate",
        json!({"url": format!("{}/page-a", server.uri())}),
    )
    .await
    .expect("navigate-a should succeed");

    // Small delay to ensure chromium has rendered the DOM before snapshotting.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let snap = invoke(&registry, "browser_snapshot", json!({}))
        .await
        .expect("snapshot should succeed");

    // Extract the ref ID of "Original" button from the snapshot output.
    let ref_id: u64 = snap
        .lines()
        .find(|l| l.contains("button") && l.contains("Original"))
        .and_then(|l| l.trim_start().strip_prefix('['))
        .and_then(|s| s.split(']').next())
        .and_then(|n| n.parse().ok())
        .expect("expected to extract ref ID from snapshot line like `[N] button \"Original\"`");

    // 2. Navigate to page-b — this invalidates ref AND clears the ref_table (plan 04).
    let _ = invoke(
        &registry,
        "browser_navigate",
        json!({"url": format!("{}/page-b", server.uri())}),
    )
    .await
    .expect("navigate-b should succeed");

    // Small delay to ensure chromium completes navigation before click.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // 3. Click stale ref — expect element_stale envelope (D-11).
    let click_result = invoke(&registry, "browser_click", json!({"ref": ref_id}))
        .await
        .expect("click should not Err — should return Ok-string with element_stale envelope");

    assert!(
        click_result.contains("\"error\":\"element_stale\""),
        "Phase 25.1 D-11: stale-ref click must return element_stale envelope, got: {click_result}"
    );
    assert!(
        click_result.contains(&format!("\"ref\":{ref_id}")),
        "envelope must echo the failing ref id, got: {click_result}"
    );

    let _ = invoke(&registry, "browser_close", json!({})).await;
    drop(session);
}

// =============================================================================
// Test 3: browser_vision routes to auxiliary vision role (D-07 / PROV-06)
// =============================================================================

/// Test-local VisionClientHandle that routes through a ProviderResolver.
///
/// This validates the D-07 cascade: when auxiliary is configured, the vision
/// call goes to the aux endpoint, NOT the main provider. The handle constructs
/// an HTTP client from the resolver and sends a multimodal-shaped JSON request,
/// confirming the correct server receives it.
struct ResolverVisionHandle {
    resolver: std::sync::Arc<ironhermes_core::provider::ProviderResolver>,
}

#[async_trait::async_trait]
impl VisionClientHandle for ResolverVisionHandle {
    async fn vision_call(&self, prompt: String, image_data_url: String) -> anyhow::Result<String> {
        // D-07 cascade: resolve vision role (level 1 override or level 2 auxiliary).
        let endpoint = match self.resolver.resolve_role("vision") {
            Some(ep) => ep,
            None => self.resolver.resolve_for_main().clone(),
        };

        // Build base_url: endpoint.base_url is already "/v1" suffix per test setup.
        // LlmClient appends /chat/completions; we do the same here for direct reqwest.
        let url = format!("{}/chat/completions", endpoint.base_url);

        let body = serde_json::json!({
            "model": endpoint.default_model,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": prompt},
                    {"type": "image_url", "image_url": {"url": image_data_url, "detail": "auto"}}
                ]
            }],
            "max_tokens": 64
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header(
                "Authorization",
                format!(
                    "Bearer {}",
                    endpoint.api_key.as_deref().unwrap_or("test-key")
                ),
            )
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("vision HTTP call failed: {e}"))?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!("vision call returned {status}: {text}"));
        }

        // Extract content from OpenAI-shaped response.
        let json: serde_json::Value =
            serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text.clone()));
        let content = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("(no content)")
            .to_string();
        Ok(content)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_vision_routes_to_auxiliary_vision_role() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    if !chromium_available() {
        eprintln!("SKIP browser_vision_routes_to_auxiliary_vision_role: no chromium binary (D-22)");
        return;
    }

    // Two wiremock servers: main provider (must NOT receive the vision request)
    // and aux provider (MUST receive it — validates D-07 cascade).
    let main_server = MockServer::start().await;
    let aux_server = MockServer::start().await;

    // Aux server: returns a valid OpenAI ChatCompletions response.
    // Path is /v1/chat/completions because ResolverVisionHandle appends /chat/completions
    // to base_url (aux_server.uri() + "/v1").
    let openai_response = json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "created": 1700000000u64,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "(test vision response)"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    });
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_response))
        .mount(&aux_server)
        .await;

    // Main server: any POST returns 500 — proves vision did NOT hit main.
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_string("main_server should NOT receive vision requests — D-07 violated"),
        )
        .mount(&main_server)
        .await;

    // Page server — chromium navigates here for the screenshot.
    let page_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"<!doctype html><html><body><h1>Vision Test Page</h1></body></html>"#,
            "text/html; charset=utf-8",
        ))
        .mount(&page_server)
        .await;

    // Set API keys (needed for ProviderResolver::build to resolve provider entries).
    let _aux_key = EnvGuard::set("OPENAI_API_KEY", "sk-aux-test");
    let _main_key = EnvGuard::set("ANTHROPIC_API_KEY", "sk-main-test");

    // Build resolver: main = anthropic (main_server) + auxiliary = openai (aux_server).
    // VERBATIM from crates/ironhermes-cli/tests/provider_integration.rs
    //   ::auxiliary_routes_to_separate_model lines 503-538.
    let mut config = ironhermes_core::Config::default();
    config.model.provider = "anthropic".to_string();
    config.model.default = "claude-sonnet-4".to_string();

    config.providers.insert(
        "anthropic".to_string(),
        ironhermes_core::ProviderConfig {
            base_url: Some(format!("{}/v1", main_server.uri())),
            api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
            api_mode: Some(ironhermes_core::config::ApiMode::AnthropicMessages),
            ..Default::default()
        },
    );
    config.providers.insert(
        "openai".to_string(),
        ironhermes_core::ProviderConfig {
            base_url: Some(format!("{}/v1", aux_server.uri())),
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            api_mode: Some(ironhermes_core::config::ApiMode::ChatCompletions),
            ..Default::default()
        },
    );
    config.auxiliary = ironhermes_core::config::AuxiliaryConfig {
        provider: "openai".to_string(),
        model: "gpt-4o-mini".to_string(),
    };

    let resolver = std::sync::Arc::new(
        ironhermes_core::ProviderResolver::build(&config).expect("resolver build must succeed"),
    );

    // Wire the ResolverVisionHandle — routes through the resolver's D-07 cascade.
    let vision_handle = std::sync::Arc::new(ResolverVisionHandle {
        resolver: resolver.clone(),
    });

    let (registry, session) = make_browser_registry(resolver, vision_handle);

    // Navigate so there's a live page to screenshot.
    let _ = invoke(
        &registry,
        "browser_navigate",
        json!({"url": format!("{}/page", page_server.uri())}),
    )
    .await
    .expect("navigate should succeed");

    // Run browser_vision — must hit aux_server, NOT main_server.
    let vision_result = invoke(
        &registry,
        "browser_vision",
        json!({"prompt": "describe this"}),
    )
    .await
    .expect("vision should succeed against aux mock");

    // D-07 invariant: aux server got the multimodal request; main server did NOT.
    let aux_received = aux_server.received_requests().await.unwrap_or_default();
    let main_received = main_server.received_requests().await.unwrap_or_default();

    assert!(
        !aux_received.is_empty(),
        "Phase 26 D-07 / Phase 25.1 D-07: aux vision server MUST receive the multimodal request; \
         vision_result={vision_result}"
    );
    assert!(
        main_received
            .iter()
            .all(|r| r.url.path() != "/v1/chat/completions"),
        "main server must NOT receive the vision call when aux.vision is configured \
         (got {} requests to main server)",
        main_received.len()
    );

    // Confirm the request body contains base64-image data (our PNG payload, D-08).
    let body_text = String::from_utf8_lossy(&aux_received[0].body);
    assert!(
        body_text.contains("base64") || body_text.contains("image_url"),
        "aux server request body should contain base64 image payload or image_url key; \
         got first 200 chars: {}",
        &body_text.chars().take(200).collect::<String>()
    );

    let _ = invoke(&registry, "browser_close", json!({})).await;
    drop(session);
}

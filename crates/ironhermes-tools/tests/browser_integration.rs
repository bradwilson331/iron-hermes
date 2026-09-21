//! Phase 25.1 D-20: Three mandatory integration tests for the browser toolset.
//! Phase 53 Plan 05 (D-08): the suite is now parameterized over backend
//! (Chromium / Obscura) — each case is ONE shared body function, called once
//! per backend, rather than a copied test. Each backend skips independently
//! (D-22's existing idiom, extended); a machine with neither binary still
//! goes fully green. This suite is validated against Obscura commit
//! `727cc46` (the commit ADR-0005 pinned, and the one the operator's local
//! `obscura` binary self-reports as `obscura 0.1.0-dev+727cc46`) — see
//! `get_layout_metrics_still_returns_floats_upstream_727cc46` below, whose
//! doc comment explains what re-pinning this suite to a newer commit means.
//!
//! Pattern: in-process ToolRegistry + chromiumoxide + wiremock-hosted local HTML.
//! Skips (not fails) when a backend's binary is unavailable (D-22, extended to Obscura).
//!
//! Test invocation: option (c) — direct in-process tool invocation via ToolRegistry
//! (no subprocess/LLM mocking required; tools are called with literal args).

use std::sync::OnceLock;

use ironhermes_core::config::BrowserBackend;
use ironhermes_tools::browser_session::{BrowserSession, find_chromium_binary, find_obscura_binary};
use ironhermes_tools::browser_vision::VisionClientHandle;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// =============================================================================
// Process-wide env lock (mirrors provider_integration.rs / toolset_integration.rs)
// =============================================================================

fn env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
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

/// Phase 53 Plan 05: the Obscura twin of [`chromium_available`]. Resolves the
/// binary the exact same way `BrowserSession::spawn`'s Obscura arm does
/// (`find_obscura_binary`, which itself checks `OBSCURA_PATH` internally), so
/// this test suite's notion of "present" cannot drift from the runtime's.
/// Honours the same `IRONHERMES_BROWSER_TEST_DISABLE` escape hatch.
fn obscura_available() -> bool {
    if std::env::var("IRONHERMES_BROWSER_TEST_DISABLE").is_ok() {
        return false;
    }
    find_obscura_binary(None).is_some()
}

// =============================================================================
// Registry construction helper
// =============================================================================

type BrowserSessionArc = std::sync::Arc<tokio::sync::Mutex<Option<BrowserSession>>>;

/// Build a ToolRegistry with all 11 browser tools registered, configured for
/// the given backend.
///
/// `vision_client` is wired into BrowserVisionTool. For tests 1 and 2, pass a
/// `NoOpVisionHandle` (they never invoke browser_vision). For test 3, pass a
/// real TestVisionHandle pointing at the aux wiremock server.
///
/// Phase 53 Plan 05: extended with a `backend` parameter rather than
/// duplicated per-backend. The Chromium arm is byte-identical to pre-53 —
/// its `no_sandbox`/`user_data_dir` fiddling is Chromium-only and the
/// Obscura arm does not need it. The Obscura arm sets `backend` and
/// `obscura_path` (resolved via the same `find_obscura_binary` the runtime
/// uses) instead.
fn make_browser_registry(
    backend: BrowserBackend,
    resolver: std::sync::Arc<ironhermes_core::provider::ProviderResolver>,
    vision_client: std::sync::Arc<dyn VisionClientHandle>,
) -> (ironhermes_tools::ToolRegistry, BrowserSessionArc) {
    let session: BrowserSessionArc = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let mut registry = ironhermes_tools::ToolRegistry::new();
    let mut config = ironhermes_core::config::Config::default();
    match backend {
        BrowserBackend::Chromium => {
            // CI (Ubuntu 23.10+) disables unprivileged user namespaces via AppArmor, which
            // causes Chromium's zygote sandbox to abort (zygote_host_impl_linux.cc: "No
            // usable sandbox!"). Set no_sandbox=true when running under CI so Chromium can
            // launch. This is test-only; the production default (no_sandbox: false) is
            // never changed.
            if std::env::var("CI").is_ok() {
                config.browser.no_sandbox = true;
            }
            // Per-invocation unique Chromium profile dir. Without this, every browser test
            // shares the default `$IRONHERMES_HOME/browser-profile`, and nextest (which runs
            // each test as a separate parallel PROCESS) makes two launches collide on
            // Chrome's per-profile SingletonLock ("Failed to create .../SingletonLock: File
            // exists"). Using an absolute path under the OS temp dir keyed by pid + an
            // atomic counter guarantees uniqueness across BOTH processes and concurrent
            // calls within one process. The path is a plain string (not a dropped TempDir),
            // so it can never be deleted out from under chromium mid-test;
            // BrowserSession::spawn create_dir_all's it on launch. Test-only — production
            // still uses the default profile dir.
            static PROFILE_SEQ: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let seq = PROFILE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let unique_profile = std::env::temp_dir()
                .join(format!("ih-browser-test-{}-{}", std::process::id(), seq));
            config.browser.user_data_dir = Some(unique_profile.to_string_lossy().into_owned());
        }
        BrowserBackend::Obscura => {
            config.browser.backend = BrowserBackend::Obscura;
            config.browser.obscura_path =
                find_obscura_binary(None).map(|p| p.to_string_lossy().into_owned());
            // D-04: Obscura's own SSRF guard denies loopback/private-network targets by
            // default, and every fixture page in this suite is a wiremock server bound to
            // 127.0.0.1. Test-only — production default (false) is never changed; the
            // Plan 02 tracer test established this same workaround for the same reason.
            config.browser.obscura_allow_private_network = true;
        }
    }
    let config = std::sync::Arc::new(config);
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

/// Phase 53 Plan 05: shared body for [`browser_navigate_then_snapshot_returns_refs`],
/// called once per backend by that test's wrapper.
async fn browser_navigate_then_snapshot_returns_refs_body(backend: BrowserBackend) {
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
    let (registry, session) = make_browser_registry(backend, resolver, noop);

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

/// Phase 53 Plan 05: parameterized wrapper — calls the shared body once per
/// backend, each independently skipped (D-22 idiom, extended to Obscura)
/// rather than failing when that backend's binary is absent.
#[tokio::test(flavor = "multi_thread")]
async fn browser_navigate_then_snapshot_returns_refs() {
    let _g = env_lock().lock().await;
    if chromium_available() {
        browser_navigate_then_snapshot_returns_refs_body(BrowserBackend::Chromium).await;
    } else {
        eprintln!(
            "SKIP browser_navigate_then_snapshot_returns_refs (chromium): no chromium binary (D-22)"
        );
    }
    if obscura_available() {
        browser_navigate_then_snapshot_returns_refs_body(BrowserBackend::Obscura).await;
    } else {
        eprintln!("SKIP browser_navigate_then_snapshot_returns_refs (obscura): no obscura binary");
    }
}

// =============================================================================
// Test 2: stale ref → element_stale envelope (D-11)
// =============================================================================

/// Phase 53 Plan 05: shared body for [`browser_click_with_stale_ref_returns_structured_error`],
/// called once per backend by that test's wrapper.
async fn browser_click_with_stale_ref_returns_structured_error_body(backend: BrowserBackend) {
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
    let (registry, session) = make_browser_registry(backend, resolver, noop);

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

/// Phase 53 Plan 05: parameterized wrapper — calls the shared body once per
/// backend, each independently skipped when that backend's binary is absent.
#[tokio::test(flavor = "multi_thread")]
async fn browser_click_with_stale_ref_returns_structured_error() {
    let _g = env_lock().lock().await;
    if chromium_available() {
        browser_click_with_stale_ref_returns_structured_error_body(BrowserBackend::Chromium).await;
    } else {
        eprintln!(
            "SKIP browser_click_with_stale_ref_returns_structured_error (chromium): no chromium \
             binary (D-22)"
        );
    }
    if obscura_available() {
        browser_click_with_stale_ref_returns_structured_error_body(BrowserBackend::Obscura).await;
    } else {
        eprintln!(
            "SKIP browser_click_with_stale_ref_returns_structured_error (obscura): no obscura \
             binary"
        );
    }
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

/// Phase 53 Plan 05: shared body for [`browser_vision_routes_to_auxiliary_vision_role`],
/// called once per backend by that test's wrapper. The capture kind
/// (full_page vs viewport) differs per backend, but this test only asserts
/// on request ROUTING (which server received the call) and payload shape
/// (base64/image_url present), both backend-independent.
async fn browser_vision_routes_to_auxiliary_vision_role_body(backend: BrowserBackend) {
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

    let (registry, session) = make_browser_registry(backend, resolver, vision_handle);

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
        body_text.chars().take(200).collect::<String>()
    );

    let _ = invoke(&registry, "browser_close", json!({})).await;
    drop(session);
}

/// Phase 53 Plan 05: parameterized wrapper — calls the shared body once per
/// backend, each independently skipped when that backend's binary is absent.
#[tokio::test(flavor = "multi_thread")]
async fn browser_vision_routes_to_auxiliary_vision_role() {
    let _g = env_lock().lock().await;
    if chromium_available() {
        browser_vision_routes_to_auxiliary_vision_role_body(BrowserBackend::Chromium).await;
    } else {
        eprintln!(
            "SKIP browser_vision_routes_to_auxiliary_vision_role (chromium): no chromium binary \
             (D-22)"
        );
    }
    if obscura_available() {
        browser_vision_routes_to_auxiliary_vision_role_body(BrowserBackend::Obscura).await;
    } else {
        eprintln!("SKIP browser_vision_routes_to_auxiliary_vision_role (obscura): no obscura binary");
    }
}

// =============================================================================
// Test 4: D-08's version-pinned upstream tripwire
// =============================================================================

/// Phase 53 Plan 05 (D-08): pins Obscura commit `727cc46` — the commit
/// ADR-0005 pinned, and the exact commit the operator's local
/// `/Users/you/code/obscura/target/release/obscura` binary self-reports
/// (`obscura 0.1.0-dev+727cc46`). `Page.getLayoutMetrics` answers
/// `clientWidth`/`clientHeight` as raw `f64` while `chromiumoxide_cdp`
/// 0.9.1 types `LayoutViewport`'s fields as `i64`, so chromiumoxide's typed
/// `Page::layout_metrics()` call rejects the reply during deserialization —
/// this is the exact rejection point `browser_vision`'s Obscura
/// viewport-only concession (Task 1's `wants_full_page`) exists to work
/// around. No raw-websocket or new-dependency CDP client is needed: the
/// already-present chromiumoxide 0.9.1's typed call is itself the signal.
///
/// Upstream `main` at `4b7028830` already carries a `coord_value()` helper
/// that fixes the IDENTICAL bug class for a sibling method —
/// `DOM.getBoxModel` / `DOM.getContentQuads` (issue #576, filed by this
/// project, credited to "Hermes Agent" in its own fix commit) — but has NOT
/// applied that treatment to `Page.getLayoutMetrics` as of that commit.
/// Because `browser_click` drives content quads, a click failure observed
/// against a binary at or after that fix may be the ALREADY-FIXED-upstream
/// #576 bug rather than a defect in our code — a reader debugging a click
/// failure needs that pointer, not a fresh investigation.
///
/// WHEN THIS TEST STARTS FAILING (i.e. `layout_metrics()` starts returning
/// `Ok`): the upstream fix for THIS method has landed. Re-evaluate
/// `browser_vision`'s viewport-only concession and Task 1's
/// `wants_full_page`, and re-derive this whole comment against the new pin
/// — do not just delete the failing assertion.
///
/// Skips (does not fail) when no Obscura binary resolves — the same
/// discipline every other case in this file follows.
#[tokio::test(flavor = "multi_thread")]
async fn get_layout_metrics_still_returns_floats_upstream_727cc46() {
    let _g = env_lock().lock().await;
    if !obscura_available() {
        eprintln!(
            "SKIP get_layout_metrics_still_returns_floats_upstream_727cc46: no obscura binary"
        );
        return;
    }

    let mut config = ironhermes_core::config::Config::default();
    config.browser.backend = BrowserBackend::Obscura;
    config.browser.obscura_path =
        find_obscura_binary(None).map(|p| p.to_string_lossy().into_owned());

    let session = BrowserSession::spawn(&config.browser)
        .await
        .expect("a render-capable obscura session should spawn for the tripwire test");

    let result = session.page.layout_metrics().await;

    assert!(
        result.is_err(),
        "Page.getLayoutMetrics is expected to still fail against Obscura commit 727cc46 \
         (float clientWidth/clientHeight vs chromiumoxide_cdp 0.9.1's i64-typed \
         LayoutViewport). If this now passes, the upstream fix for getLayoutMetrics has \
         landed — revisit browser_vision's viewport-only concession (Task 1's \
         wants_full_page) and this test's own doc comment; do NOT just delete this \
         assertion. Got: {result:?}"
    );

    // Never leak the locally-spawned `obscura serve` process (T-53-05-03).
    let _ = session.close().await;
}

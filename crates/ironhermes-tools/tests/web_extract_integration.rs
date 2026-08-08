//! Phase 25.2 D-26 + D-27: wiremock-backed integration tests for web_extract.
//!
//! Tests in this file (added in plan 25.2-13):
//!   1. web_extract_single_url_local_fallback_returns_markdown      (D-26 #1, D-04)
//!   2. web_extract_pdf_url_routes_to_pdf_backend                   (D-26 #2, D-09)
//!   3. web_extract_youtube_url_dispatches_to_skill                 (D-26 #3, D-10)
//!   4. web_extract_summary_tier_thresholds                         (D-26 #4, D-11)
//!   5. web_extract_use_llm_processing_false_skips_all_aux_calls    (D-26 #5, D-12)
//!   6. web_extract_summarization_role_resolves_via_phase26_cascade (D-26 #6, D-13)
//!   7. web_extract_secret_in_url_redacted                          (D-26 #7, D-19)
//!   8. web_extract_multi_url_partial_failure_returns_per_url_errors (D-26 #8, D-02)
//!   9. web_extract_excluded_when_no_backend_available              (D-27 — schema-availability)
//!
//! All tests use wiremock (no live network). env_lock + EnvGuard are lifted from
//! crates/ironhermes-tools/tests/browser_integration.rs (Phase 25.1).

#![allow(dead_code)] // Helpers used by tests added in plan 25.2-14.

use std::sync::OnceLock;

/// Process-wide lock for env-var mutation in tests (Rust 2024 makes set_var unsafe).
/// Source: crates/ironhermes-tools/tests/browser_integration.rs:21
pub(crate) fn env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// RAII guard that restores the previous env var value on drop.
/// Source: crates/ironhermes-tools/tests/browser_integration.rs:25-53
pub(crate) struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    pub(crate) fn set(key: &'static str, val: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: Rust 2024 set_var; tests serialised by env_lock().
        unsafe { std::env::set_var(key, val) };
        Self { key, prev }
    }

    pub(crate) fn unset(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: Rust 2024 remove_var; tests serialised by env_lock().
        unsafe { std::env::remove_var(key) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: serialised by env_lock(); restoring prior state.
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

// =============================================================================
// Plan 25.2-13 Task 1 — D-26 + D-27 integration tests
// =============================================================================

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use ironhermes_core::{SkillRegistry, SummarizationClientHandle};
use ironhermes_tools::ToolRegistry;

// -----------------------------------------------------------------------------
// CountingSummarizer — captures every aux LLM call without doing real I/O.
// Used by tests #4, #5, #7, #8 to assert *zero* aux calls were made (or to
// confirm tier-2 fired exactly once for the cost-amplification gate).
// -----------------------------------------------------------------------------
pub(crate) struct CountingSummarizer {
    pub calls: Mutex<Vec<(String, String, u32)>>, // (system, user_prefix, max_tokens)
    pub response: String,
}

#[async_trait]
impl SummarizationClientHandle for CountingSummarizer {
    async fn summarize_call(
        &self,
        system_prompt: String,
        user_prompt: String,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        let user_prefix: String = user_prompt.chars().take(50).collect();
        self.calls
            .lock()
            .unwrap()
            .push((system_prompt, user_prefix, max_tokens));
        Ok(self.response.clone())
    }
}

// -----------------------------------------------------------------------------
// Stub youtube-content skill registry (test #3)
// -----------------------------------------------------------------------------

/// Build a SkillRegistry rooted in a tempdir with a stub `youtube-content` skill.
/// The stub script prints canned Markdown so the YouTube dispatch path produces
/// deterministic content without hitting the network.
///
/// `SkillRegistry::load_with_paths` walks immediate children of each search-path
/// looking for `<child>/SKILL.md`, so the layout MUST be
/// `<tmp>/youtube-content/SKILL.md` (not `<tmp>/media/youtube-content/SKILL.md`).
pub(crate) fn make_skill_registry_with_stub_youtube() -> (Arc<SkillRegistry>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let skill_dir = tmp.path().join("youtube-content");
    std::fs::create_dir_all(skill_dir.join("scripts")).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: youtube-content\ndescription: stub for tests\n---\n",
    )
    .unwrap();
    std::fs::write(
        skill_dir.join("scripts/fetch_transcript.py"),
        "#!/usr/bin/env python3\nprint('# Test Video\\n\\nMock transcript line 1.\\nMock transcript line 2.')\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = skill_dir.join("scripts/fetch_transcript.py");
        let mut perms = std::fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&p, perms).unwrap();
    }

    // Use load_with_paths so the search-path is exactly tmp.path() — load() would
    // also consult get_hermes_home()/skills, polluting the test registry with the
    // operator's installed skills.
    let registry = SkillRegistry::load_with_paths(&[tmp.path().to_path_buf()]);
    (Arc::new(registry), tmp)
}

// -----------------------------------------------------------------------------
// make_test_registry — ToolRegistry wired with WebExtractTool + CountingSummarizer
// + stub youtube-content skill. Returned tempdir handle keeps the skill tree alive.
// -----------------------------------------------------------------------------
pub(crate) fn make_test_registry() -> (ToolRegistry, Arc<CountingSummarizer>, tempfile::TempDir) {
    let counter = Arc::new(CountingSummarizer {
        calls: Mutex::new(Vec::new()),
        response: "MOCK_SUMMARY".to_string(),
    });
    let handle: Arc<dyn SummarizationClientHandle> = counter.clone();
    let (skill_reg, tmp) = make_skill_registry_with_stub_youtube();
    let mut reg = ToolRegistry::new();
    reg.register_web_extract_tool(handle, skill_reg);
    (reg, counter, tmp)
}

// -----------------------------------------------------------------------------
// Minimal valid PDF byte literal (~480 bytes). pdf-extract 0.10 accepts this
// stripped-down PDF; if it ever rejects an even-more-minimal variant, swap to
// a fixture file via include_bytes!("fixtures/sample.pdf").
// -----------------------------------------------------------------------------
static MIN_PDF: &[u8] = b"%PDF-1.4\n1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n2 0 obj\n<</Type/Pages/Kids[3 0 R]/Count 1>>\nendobj\n3 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R>>\nendobj\n4 0 obj\n<</Length 44>>stream\nBT /F1 12 Tf 100 700 Td (Hello PDF World) Tj ET\nendstream\nendobj\nxref\n0 5\n0000000000 65535 f \n0000000009 00000 n \n0000000054 00000 n \n0000000098 00000 n \n0000000169 00000 n \ntrailer\n<</Size 5/Root 1 0 R>>\nstartxref\n253\n%%EOF\n";

// =============================================================================
// D-26 #1: local fallback returns Markdown
// =============================================================================
#[tokio::test(flavor = "multi_thread")]
async fn web_extract_single_url_local_fallback_returns_markdown() {
    let _g = env_lock().lock().await;
    let _f = EnvGuard::unset("FIRECRAWL_API_KEY");
    let _e = EnvGuard::unset("EXA_API_KEY");
    let _t = EnvGuard::unset("TAVILY_API_KEY");
    // SSRF test escape hatch: wiremock listens on 127.0.0.1, which is_safe_url
    // correctly blocks in production. The bypass is loopback-only and gated by
    // the _TEST_ env var (Plan 25.2-13 [Rule 3 deviation]).
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_raw(
                    r#"<html><head><title>Local Test</title></head><body><article><p>Hello local world.</p></article></body></html>"#,
                    "text/html",
                ),
        )
        .mount(&server)
        .await;

    let (reg, _counter, _tmp) = make_test_registry();
    let url = format!("{}/article", server.uri());
    let result = reg
        .handle_tool_call(
            "web_extract",
            json!({ "urls": [url], "use_llm_processing": false }),
        )
        .await
        .expect("web_extract call should succeed");

    let results: Vec<Value> = serde_json::from_str(&result).expect("parse Vec<ExtractionResult>");
    assert_eq!(results.len(), 1, "exactly one ExtractionResult expected");
    assert!(
        results[0]["error"].is_null(),
        "expected error: null, got {:?}",
        results[0]["error"]
    );
    let content = results[0]["content"].as_str().unwrap_or("");
    assert!(
        content.contains("Hello local world"),
        "expected extracted body in content; got: {}",
        content
    );
}

// =============================================================================
// D-26 #2: PDF URL routes to PDF backend
// =============================================================================
#[tokio::test(flavor = "multi_thread")]
async fn web_extract_pdf_url_routes_to_pdf_backend() {
    let _g = env_lock().lock().await;
    let _f = EnvGuard::unset("FIRECRAWL_API_KEY");
    let _e = EnvGuard::unset("EXA_API_KEY");
    let _t = EnvGuard::unset("TAVILY_API_KEY");
    // SSRF test escape hatch: wiremock listens on 127.0.0.1, which is_safe_url
    // correctly blocks in production. The bypass is loopback-only and gated by
    // the _TEST_ env var (Plan 25.2-13 [Rule 3 deviation]).
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/doc.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/pdf")
                .set_body_bytes(MIN_PDF.to_vec()),
        )
        .mount(&server)
        .await;

    let (reg, _counter, _tmp) = make_test_registry();
    let url = format!("{}/doc.pdf", server.uri());
    let result = reg
        .handle_tool_call(
            "web_extract",
            json!({ "urls": [url], "use_llm_processing": false }),
        )
        .await
        .expect("web_extract call should succeed");

    let results: Vec<Value> = serde_json::from_str(&result).unwrap();
    assert_eq!(results.len(), 1);

    // PDF route succeeded if either:
    //   (a) error is null and content has text (pdf-extract returned something), OR
    //   (b) error mentions "pdf" anywhere (pdf-extract failure on the minimal PDF
    //       fixture is acceptable for this test — we are validating that the
    //       *dispatch classifier* sent us through the PDF backend, not that
    //       pdf-extract can parse our hand-rolled byte literal). The route is
    //       proven by the presence of `pdf_*` in the error envelope; the only
    //       failure case for this test is a fall-through to the web backend chain
    //       (which would surface `backend_chain_exhausted`) or any non-pdf-related
    //       error class indicating the dispatcher never sent us to extract_pdf.
    // What we MUST reject: error=="url_contains_secret", "backend_chain_exhausted",
    // or "unknown_classification" — these would indicate the dispatch classifier
    // never sent us through the PDF backend.
    let err = results[0]["error"].as_str();
    let pdf_route_taken = err.is_none()
        || err
            .map(|e| e.contains("pdf") || e == "content_below_min_length")
            .unwrap_or(false);
    assert!(
        pdf_route_taken,
        "PDF route was not taken (dispatch classifier failed to route .pdf URL): error={:?}; content={:?}",
        err, results[0]["content"]
    );
}

// =============================================================================
// D-26 #3: YouTube URL dispatches to skill (zero HTTP calls reach our wiremock)
// =============================================================================
#[tokio::test(flavor = "multi_thread")]
async fn web_extract_youtube_url_dispatches_to_skill() {
    let _g = env_lock().lock().await;
    let _f = EnvGuard::unset("FIRECRAWL_API_KEY");
    let _e = EnvGuard::unset("EXA_API_KEY");
    let _t = EnvGuard::unset("TAVILY_API_KEY");
    // SSRF test escape hatch: wiremock listens on 127.0.0.1, which is_safe_url
    // correctly blocks in production. The bypass is loopback-only and gated by
    // the _TEST_ env var (Plan 25.2-13 [Rule 3 deviation]).
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");

    // wiremock only to *prove* the YouTube path made zero requests against any
    // HTTP server we control — we never hand the URL to wiremock.
    let server = MockServer::start().await;

    let (reg, _counter, _tmp) = make_test_registry();
    let result = reg
        .handle_tool_call(
            "web_extract",
            json!({ "urls": ["https://youtu.be/abc123"], "use_llm_processing": false }),
        )
        .await
        .expect("web_extract call should succeed");

    let results: Vec<Value> = serde_json::from_str(&result).unwrap();
    assert_eq!(results.len(), 1);

    let entry = &results[0];
    let succeeded = entry["error"].is_null()
        && entry["content"]
            .as_str()
            .unwrap_or("")
            .contains("Test Video");
    let graceful_error = entry["error"]
        .as_str()
        .map(|e| e.starts_with("youtube") || e.starts_with("skill"))
        .unwrap_or(false);
    assert!(
        succeeded || graceful_error,
        "YouTube path expected to either return stub content or a graceful skill error; got: {:?}",
        entry
    );

    // CRITICAL: the YouTube classifier must NOT fall through to the web backend
    // chain — wiremock should record zero received requests.
    let received = server.received_requests().await.unwrap();
    assert!(
        received.is_empty(),
        "YouTube path made {} unexpected HTTP calls",
        received.len()
    );
}

// =============================================================================
// D-26 #4: tier thresholds (tier 4 refuses; tier 1 makes zero aux calls)
// =============================================================================
#[tokio::test(flavor = "multi_thread")]
async fn web_extract_summary_tier_thresholds() {
    let _g = env_lock().lock().await;
    let _f = EnvGuard::unset("FIRECRAWL_API_KEY");
    let _e = EnvGuard::unset("EXA_API_KEY");
    let _t = EnvGuard::unset("TAVILY_API_KEY");
    // SSRF test escape hatch: wiremock listens on 127.0.0.1, which is_safe_url
    // correctly blocks in production. The bypass is loopback-only and gated by
    // the _TEST_ env var (Plan 25.2-13 [Rule 3 deviation]).
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");

    // ---- Tier 4 (>2M): refuse, zero aux calls ----
    {
        let server = MockServer::start().await;
        let big = "a".repeat(2_500_000);
        Mock::given(method("GET"))
            .and(path("/big"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_raw(
                        format!("<html><body><article>{}</article></body></html>", big),
                        "text/html",
                    ),
            )
            .mount(&server)
            .await;

        let (reg, counter, _tmp) = make_test_registry();
        let url = format!("{}/big", server.uri());
        let result = reg
            .handle_tool_call(
                "web_extract",
                json!({ "urls": [url], "use_llm_processing": true }),
            )
            .await
            .unwrap();
        let r: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(
            r[0]["error"].as_str(),
            Some("content_too_large"),
            "tier 4 must refuse with content_too_large; got {:?}",
            r[0]
        );
        assert_eq!(
            counter.calls.lock().unwrap().len(),
            0,
            "tier 4 must make zero aux LLM calls"
        );
    }

    // ---- Tier 1 (<5K): raw, zero aux calls ----
    {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/small"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_raw(
                        "<html><body><article>tiny</article></body></html>",
                        "text/html",
                    ),
            )
            .mount(&server)
            .await;

        let (reg, counter, _tmp) = make_test_registry();
        let url = format!("{}/small", server.uri());
        let _ = reg
            .handle_tool_call(
                "web_extract",
                json!({ "urls": [url], "use_llm_processing": true }),
            )
            .await
            .unwrap();
        assert_eq!(
            counter.calls.lock().unwrap().len(),
            0,
            "tier 1 must make zero aux LLM calls (content < 5K chars)"
        );
    }
}

// =============================================================================
// D-26 #5: use_llm_processing=false skips ALL aux calls
// =============================================================================
#[tokio::test(flavor = "multi_thread")]
async fn web_extract_use_llm_processing_false_skips_all_aux_calls() {
    let _g = env_lock().lock().await;
    let _f = EnvGuard::unset("FIRECRAWL_API_KEY");
    let _e = EnvGuard::unset("EXA_API_KEY");
    let _t = EnvGuard::unset("TAVILY_API_KEY");
    // SSRF test escape hatch: wiremock listens on 127.0.0.1, which is_safe_url
    // correctly blocks in production. The bypass is loopback-only and gated by
    // the _TEST_ env var (Plan 25.2-13 [Rule 3 deviation]).
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");

    let server = MockServer::start().await;
    let big = "x".repeat(600_000);
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_raw(
                    format!("<html><body><article>{}</article></body></html>", big),
                    "text/html",
                ),
        )
        .mount(&server)
        .await;

    let (reg, counter, _tmp) = make_test_registry();
    let url = format!("{}/x", server.uri());
    let _ = reg
        .handle_tool_call(
            "web_extract",
            json!({ "urls": [url], "use_llm_processing": false }),
        )
        .await
        .unwrap();

    let n = counter.calls.lock().unwrap().len();
    assert_eq!(
        n, 0,
        "use_llm_processing=false must produce 0 aux LLM calls; got {}",
        n
    );
}

// =============================================================================
// D-26 #6: summarization role resolves via Phase 26 cascade — END-TO-END
// =============================================================================
//
// NOTE (Plan 25.2-13 [Rule 3 deviation]): The plan's prose called for using the
// real `AnyClientSummarizationHandle` from Plan 25.2-14, but Plan 14 has not
// landed yet (incomplete_plans includes 25.2-14-PLAN.md at execution time).
// To keep Plan 13 unblocked, this test uses a `Phase26CascadeHandle` that
// performs the *same* `ProviderResolver::resolve_role("summarization")` cascade
// lookup that Plan 14's handle will perform, then POSTs the request to the
// resolved endpoint via reqwest. The end-to-end assertion (cascade routes
// summary calls to the AUX server, not the MAIN server) is identical; only the
// crate boundary is different. When Plan 14 lands, this test will continue to
// hold and may be augmented (or replaced) by a sibling test that imports the
// real `AnyClientSummarizationHandle`.
#[tokio::test(flavor = "multi_thread")]
async fn web_extract_summarization_role_resolves_via_phase26_cascade() {
    use ironhermes_core::ProviderResolver;
    use ironhermes_core::config::Config;

    let _g = env_lock().lock().await;
    let _f = EnvGuard::unset("FIRECRAWL_API_KEY");
    let _e = EnvGuard::unset("EXA_API_KEY");
    let _t = EnvGuard::unset("TAVILY_API_KEY");
    // SSRF test escape hatch: wiremock listens on 127.0.0.1, which is_safe_url
    // correctly blocks in production. The bypass is loopback-only and gated by
    // the _TEST_ env var (Plan 25.2-13 [Rule 3 deviation]).
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");

    // ── Two wiremock servers ─────────────────────────────────────────────────
    // MAIN: serves the tier-2 HTML content. Must NOT receive a chat/completions call.
    let main_server = MockServer::start().await;
    let mid = "y".repeat(50_000); // tier 2: 5K-500K chars
    Mock::given(method("GET"))
        .and(path("/mid"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_raw(
                    format!("<html><body><article>{}</article></body></html>", mid),
                    "text/html",
                ),
        )
        .mount(&main_server)
        .await;
    // Sentinel mock: any chat/completions hit on MAIN is the failure case.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(599))
        .mount(&main_server)
        .await;

    // AUX: returns canned OpenAI-compatible response; we want to see this hit.
    let aux_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(
                    r#"{
                        "id": "test",
                        "object": "chat.completion",
                        "created": 0,
                        "model": "test-aux-model",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": "AUX_SUMMARY"},
                            "finish_reason": "stop"
                        }]
                    }"#,
                ),
        )
        .mount(&aux_server)
        .await;

    // ── Config with auxiliary.summary cascade pointing at AUX ────────────────
    // CustomProviderConfig is a list with `name:` field (not a map keyed by name).
    let yaml = format!(
        r#"
model:
  provider: test-main
  model: test-main-model
providers:
  test-main:
    base_url: "{main_uri}"
    api_mode: chat_completions
    default_model: test-main-model
custom_providers:
  - name: test-aux
    base_url: "{aux_uri}"
    api_mode: chat_completions
    default_model: test-aux-model
auxiliary:
  provider: test-aux
  model: test-aux-model
"#,
        main_uri = main_server.uri(),
        aux_uri = aux_server.uri(),
    );
    let config: Config = serde_yaml::from_str(&yaml).expect("test config parses");
    let resolver = Arc::new(ProviderResolver::build(&config).expect("resolver builds"));

    // ── Phase26CascadeHandle: the routing-aware summarization handle. ────────
    // Resolves "summarization" via the cascade exactly as Plan 14's
    // AnyClientSummarizationHandle will, then POSTs to the resolved endpoint.
    let handle: Arc<dyn SummarizationClientHandle> =
        Arc::new(Phase26CascadeHandle::new(resolver.clone()));

    // Empty SkillRegistry — this test does not exercise the YouTube path.
    let tmp = tempfile::tempdir().expect("tempdir");
    let skill_registry = Arc::new(SkillRegistry::load(tmp.path()));

    let mut reg = ToolRegistry::new();
    reg.register_web_extract_tool(handle, skill_registry);

    // ── Exercise web_extract on a tier-2 URL ─────────────────────────────────
    let url = format!("{}/mid", main_server.uri());
    let result = reg
        .handle_tool_call(
            "web_extract",
            json!({ "urls": [url], "use_llm_processing": true }),
        )
        .await
        .expect("web_extract call succeeds");

    let r: Vec<Value> = serde_json::from_str(&result).unwrap();
    assert_eq!(r.len(), 1);

    // ── Assertions: cascade routed summary call to AUX, not MAIN ─────────────
    assert_eq!(
        r[0]["content"].as_str(),
        Some("AUX_SUMMARY"),
        "Phase 26 cascade should have routed the summary to the AUX server (got: {:?})",
        r[0]
    );

    let aux_received = aux_server.received_requests().await.unwrap();
    let aux_chat_hits = aux_received
        .iter()
        .filter(|req| req.url.path() == "/v1/chat/completions" && req.method.as_str() == "POST")
        .count();
    assert!(
        aux_chat_hits >= 1,
        "AUX server must receive at least 1 POST /v1/chat/completions; got {}",
        aux_chat_hits
    );

    let main_received = main_server.received_requests().await.unwrap();
    let main_chat_hits = main_received
        .iter()
        .filter(|req| req.url.path() == "/v1/chat/completions" && req.method.as_str() == "POST")
        .count();
    assert_eq!(
        main_chat_hits, 0,
        "MAIN server must receive zero chat/completions calls — cascade should route summary to AUX. Got {} hits.",
        main_chat_hits
    );
}

/// Test-only summarization handle that resolves the Phase 26 cascade for the
/// `summarization` role and POSTs an OpenAI-compatible chat/completions request
/// to the resolved endpoint. Mirrors what Plan 14's AnyClientSummarizationHandle
/// will do at the agent crate boundary, without depending on agent crate types
/// (Plan 14 has not landed at Plan 13 execution time).
struct Phase26CascadeHandle {
    resolver: Arc<ironhermes_core::ProviderResolver>,
    http: reqwest::Client,
}

impl Phase26CascadeHandle {
    fn new(resolver: Arc<ironhermes_core::ProviderResolver>) -> Self {
        Self {
            resolver,
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl SummarizationClientHandle for Phase26CascadeHandle {
    async fn summarize_call(
        &self,
        system_prompt: String,
        user_prompt: String,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        // D-13: cascade exactly like Plan 14's handle will.
        let endpoint = self
            .resolver
            .resolve_role("summarization")
            .unwrap_or_else(|| self.resolver.resolve_for_main().clone());

        let url = format!(
            "{}/v1/chat/completions",
            endpoint.base_url.trim_end_matches('/')
        );
        let body = json!({
            "model": endpoint.default_model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt},
            ],
            "max_tokens": max_tokens,
        });

        let resp = self.http.post(&url).json(&body).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("summarization endpoint returned {}: {}", status, text);
        }

        let v: Value = serde_json::from_str(&text)?;
        let content = v
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!("no message.content in summarization response: {}", text)
            })?
            .to_string();
        Ok(content)
    }
}

// =============================================================================
// D-26 #7: secret in URL redacted (zero network calls)
// =============================================================================
#[tokio::test(flavor = "multi_thread")]
async fn web_extract_secret_in_url_redacted() {
    let _g = env_lock().lock().await;

    let server = MockServer::start().await; // intentionally no mocks: any hit would 404

    let (reg, _counter, _tmp) = make_test_registry();
    let url = format!("{}/api?token=SECRET123", server.uri());
    let result = reg
        .handle_tool_call(
            "web_extract",
            json!({ "urls": [url], "use_llm_processing": false }),
        )
        .await
        .unwrap();

    let r: Vec<Value> = serde_json::from_str(&result).unwrap();
    assert_eq!(r[0]["error"].as_str(), Some("url_contains_secret"));
    assert!(
        r[0]["content"].as_str().unwrap_or("").is_empty(),
        "content must be empty on secret-redaction error"
    );

    // CRITICAL: zero HTTP calls reached the server.
    let received = server.received_requests().await.unwrap();
    assert!(
        received.is_empty(),
        "secret URL leaked: {} HTTP calls made",
        received.len()
    );
}

// =============================================================================
// D-26 #8: multi-URL partial failure preserves order + per-URL errors
// =============================================================================
#[tokio::test(flavor = "multi_thread")]
async fn web_extract_multi_url_partial_failure_returns_per_url_errors() {
    let _g = env_lock().lock().await;
    let _f = EnvGuard::unset("FIRECRAWL_API_KEY");
    let _e = EnvGuard::unset("EXA_API_KEY");
    let _t = EnvGuard::unset("TAVILY_API_KEY");
    // SSRF test escape hatch: wiremock listens on 127.0.0.1, which is_safe_url
    // correctly blocks in production. The bypass is loopback-only and gated by
    // the _TEST_ env var (Plan 25.2-13 [Rule 3 deviation]).
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ok1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_raw(
                    "<html><body><article>OK ONE</article></body></html>",
                    "text/html",
                ),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/fail"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/ok2"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_raw(
                    "<html><body><article>OK TWO</article></body></html>",
                    "text/html",
                ),
        )
        .mount(&server)
        .await;

    let (reg, _counter, _tmp) = make_test_registry();
    let urls = vec![
        format!("{}/ok1", server.uri()),
        format!("{}/fail", server.uri()),
        format!("{}/ok2", server.uri()),
    ];
    let result = reg
        .handle_tool_call(
            "web_extract",
            json!({ "urls": urls, "use_llm_processing": false }),
        )
        .await
        .unwrap();

    let r: Vec<Value> = serde_json::from_str(&result).unwrap();
    assert_eq!(r.len(), 3, "ordering preservation: 3 results expected");

    // RESEARCH.md Pitfall 6: ordering preservation — successful URLs at indices 0 and 2.
    assert!(
        r[0]["content"].as_str().unwrap_or("").contains("OK ONE"),
        "first URL succeeded: {:?}",
        r[0]
    );
    assert!(
        r[1]["error"].as_str().is_some(),
        "middle URL must populate error envelope: {:?}",
        r[1]
    );
    assert!(
        r[2]["content"].as_str().unwrap_or("").contains("OK TWO"),
        "third URL succeeded: {:?}",
        r[2]
    );
}

// =============================================================================
// D-27: schema is ALWAYS present regardless of env-var state
// =============================================================================
#[tokio::test(flavor = "multi_thread")]
async fn web_extract_excluded_when_no_backend_available() {
    // Despite the test name (lifted verbatim from VALIDATION.md), the assertion
    // is that web_extract is NOT excluded — D-27 says it's ALWAYS available
    // because the local backend has no env-var prereq.
    let _g = env_lock().lock().await;
    let _f = EnvGuard::unset("FIRECRAWL_API_KEY");
    let _e = EnvGuard::unset("EXA_API_KEY");
    let _t = EnvGuard::unset("TAVILY_API_KEY");
    // SSRF test escape hatch: wiremock listens on 127.0.0.1, which is_safe_url
    // correctly blocks in production. The bypass is loopback-only and gated by
    // the _TEST_ env var (Plan 25.2-13 [Rule 3 deviation]).
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");

    let (reg, _counter, _tmp) = make_test_registry();
    let definitions = reg.get_definitions(None);
    let has_web_extract = definitions.iter().any(|d| d.function.name == "web_extract");
    assert!(
        has_web_extract,
        "D-27: web_extract must appear in definitions even with no provider env vars"
    );
}

// =============================================================================
// Phase 41.3 Plan 03 Task 1 (D-04/D-16): registry-budget arithmetic guard.
// =============================================================================

/// `web_extract`'s declared registry budget (Plan 01's `Tool::timeout_secs()`)
/// must strictly exceed the worst-case batch time implied by D-16's per-URL
/// deadline and `extract.max_parallel_summaries`, or the registry ceiling could
/// pre-empt a batch the per-URL deadlines would otherwise have completed.
/// Encodes the arithmetic comment on `WebExtractTool::timeout_secs()` as an
/// executable constraint rather than a comment only.
#[tokio::test(flavor = "multi_thread")]
async fn web_extract_budget_exceeds_worst_case_batch() {
    let (reg, _counter, _tmp) = make_test_registry();
    let tool = reg
        .get("web_extract")
        .expect("web_extract must be registered");
    let budget = tool
        .timeout_secs()
        .expect("web_extract must declare a registry budget (D-04)");

    let cfg = ironhermes_core::config::Config::default();
    const MAX_REASONABLE_BATCH: u64 = 8;
    let rounds = MAX_REASONABLE_BATCH.div_ceil(cfg.extract.max_parallel_summaries as u64);
    let worst_case_batch_secs = cfg.extract.per_url_timeout_secs * rounds;

    assert!(
        budget > worst_case_batch_secs,
        "web_extract's declared timeout_secs() ({budget}s) must exceed the worst-case \
         {MAX_REASONABLE_BATCH}-URL batch time ({worst_case_batch_secs}s = \
         {}s per-URL * {rounds} rounds at parallelism {}), or the registry ceiling \
         (Plan 01) could pre-empt a batch the per-URL deadlines would have completed",
        cfg.extract.per_url_timeout_secs, cfg.extract.max_parallel_summaries
    );
}

// =============================================================================
// Phase 41.3 Plan 03 Task 2 (D-16): the blackbox 0eaed980 regression test.
// =============================================================================

/// `partial_batch_on_one_stuck_url` — the direct regression test for blackbox
/// run `0eaed980`. A 6-URL batch where url index 3 is a REAL delayed HTTP
/// response (wiremock `set_delay`, not a synthetic `tokio::time::sleep` future —
/// 41.3-VALIDATION.md Anti-Self-Verification Guard 1 / RESEARCH.md Pitfall 4)
/// must return `Ok` with all 6 results, in input order, the stuck URL's entry
/// carrying an `extraction_timeout` error at its ORIGINAL index — not appended
/// at the end — and complete in a fraction of the 120s delay. Drives the real
/// `WebExtractTool::execute` entry point via `ToolRegistry::handle_tool_call`,
/// not `process_one_url` in isolation.
#[tokio::test(flavor = "multi_thread")]
async fn partial_batch_on_one_stuck_url() {
    let _g = env_lock().lock().await;
    let _f = EnvGuard::unset("FIRECRAWL_API_KEY");
    let _e = EnvGuard::unset("EXA_API_KEY");
    let _t = EnvGuard::unset("TAVILY_API_KEY");
    // SSRF test escape hatch: wiremock listens on 127.0.0.1, which is_safe_url
    // correctly blocks in production. The bypass is loopback-only and gated by
    // the _TEST_ env var (Plan 25.2-13 [Rule 3 deviation]).
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");

    // Redirect Config::load() to a tempdir with extract.per_url_timeout_secs
    // lowered to 2s (the shipped default is 60s — too slow for a test to wait
    // out even on the "fast" URLs' round). Mirrors the IRONHERMES_HOME +
    // written config.yaml convention already used by
    // crates/ironhermes-tools/tests/cronjob_tool_default_deliver.rs.
    let cfg_tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        cfg_tmp.path().join("config.yaml"),
        "extract:\n  per_url_timeout_secs: 2\n",
    )
    .expect("write test config.yaml");
    let _home = EnvGuard::set(
        "IRONHERMES_HOME",
        cfg_tmp.path().to_str().expect("tmp path is valid utf8"),
    );

    let server = MockServer::start().await;
    for i in [0usize, 1, 2, 4, 5] {
        let marker = format!("OK{i}");
        Mock::given(method("GET"))
            .and(path(format!("/url{i}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_raw(
                        format!("<html><body><article>{marker}</article></body></html>"),
                        "text/html",
                    ),
            )
            .mount(&server)
            .await;
    }
    // url3: a REAL delayed HTTP response, far past the 2s per-URL deadline —
    // reproducing the bot-walled news homepage from blackbox run 0eaed980.
    Mock::given(method("GET"))
        .and(path("/url3"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(120)))
        .mount(&server)
        .await;

    let (reg, _counter, _tmp) = make_test_registry();
    let urls: Vec<String> = (0..6)
        .map(|i| format!("{}/url{}", server.uri(), i))
        .collect();

    let start = Instant::now();
    let result = reg
        .handle_tool_call(
            "web_extract",
            json!({ "urls": urls, "use_llm_processing": false }),
        )
        .await
        .expect("web_extract must return Ok, not Err, when one URL hangs (D-16)");
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "the 6-URL batch must complete well under the 120s stuck-URL delay; took {elapsed:?}"
    );

    let results: Vec<Value> = serde_json::from_str(&result).expect("parse Vec<ExtractionResult>");
    assert_eq!(results.len(), 6, "exactly 6 ExtractionResult entries expected");

    for i in [0usize, 1, 2, 4, 5] {
        let marker = format!("OK{i}");
        assert!(
            results[i]["content"].as_str().unwrap_or("").contains(&marker),
            "index {i} expected content containing {marker}; got {:?}",
            results[i]
        );
    }

    // Pitfall 6 / D-16: the stuck URL's error entry sits at its ORIGINAL input
    // index (3), not appended at the end of the array.
    let stuck_error = results[3]["error"].as_str().unwrap_or("");
    assert!(
        stuck_error.contains("extraction_timeout"),
        "index 3 (the stuck URL) must carry an extraction_timeout error at its \
         original index; got {:?}",
        results[3]
    );
    assert!(
        results[3]["content"].as_str().unwrap_or("").is_empty(),
        "stuck entry must have empty content: {:?}",
        results[3]
    );
}

/// `stuck_url_error_entry_does_not_leak_the_request_url_query`.
///
/// Mechanism note (documented rather than silently assumed): `sanitize::contains_secret`
/// runs synchronously as `process_one_url`'s FIRST step, before any `.await` point. A
/// URL matching a `SECRET_URL_PATTERNS` entry (e.g. `token=`) can therefore NEVER reach
/// the network fetch this plan's new per-URL `tokio::time::timeout` wraps — it is
/// rejected by the D-19 pre-gate on the task future's very first poll, before the
/// timeout race can even begin. A URL carrying both a recognized secret AND a hang
/// is intercepted by the pre-gate, not the new D-16 timeout arm.
///
/// This test proves the invariant the plan's `must_haves` actually require — the
/// query string never reaches the model-visible error entry, and zero HTTP calls
/// reach the (deliberately hanging) mock server — regardless of which of the two
/// gates intercepts it. It is deliberately NOT a test of the D-16 timeout arm's own
/// `redact_secrets_in_url` call (that call is only reachable via a URL with no
/// recognized secret pattern — see Task 1's placement inside the spawn block — and
/// the D-16 arm's use of `redact_secrets_in_url` in the `tracing::warn!` is instead
/// covered by Task 1's acceptance criteria via `git diff` grep).
#[tokio::test(flavor = "multi_thread")]
async fn stuck_url_error_entry_does_not_leak_the_request_url_query() {
    let _g = env_lock().lock().await;
    let _f = EnvGuard::unset("FIRECRAWL_API_KEY");
    let _e = EnvGuard::unset("EXA_API_KEY");
    let _t = EnvGuard::unset("TAVILY_API_KEY");
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");

    let cfg_tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        cfg_tmp.path().join("config.yaml"),
        "extract:\n  per_url_timeout_secs: 2\n",
    )
    .expect("write test config.yaml");
    let _home = EnvGuard::set(
        "IRONHERMES_HOME",
        cfg_tmp.path().to_str().expect("tmp path is valid utf8"),
    );

    let server = MockServer::start().await;
    // Deliberately hanging — a request reaching this mock at all would mean the
    // D-19 secret pre-gate did NOT short-circuit before the network attempt.
    Mock::given(method("GET"))
        .and(path("/stuck"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(120)))
        .mount(&server)
        .await;

    let secret_query = "token=SUPER_SECRET_QUERY_VALUE_9x7z";
    let stuck_url = format!("{}/stuck?{secret_query}", server.uri());

    let (reg, _counter, _tmp) = make_test_registry();
    let start = Instant::now();
    let result = reg
        .handle_tool_call(
            "web_extract",
            json!({ "urls": [stuck_url], "use_llm_processing": false }),
        )
        .await
        .expect("web_extract must return Ok");
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(10),
        "the secret-URL pre-gate must short-circuit before any network attempt \
         (well before the 120s delay, and before the 2s per-URL deadline too); \
         took {elapsed:?}"
    );
    assert!(
        !result.contains("SUPER_SECRET_QUERY_VALUE_9x7z"),
        "the raw secret value must never appear anywhere in the serialized \
         ExtractionResult array; got: {result}"
    );

    let received = server.received_requests().await.unwrap();
    assert!(
        received.is_empty(),
        "the secret-URL pre-gate must reject the URL before any HTTP call reaches \
         the (deliberately hanging) mock server; got {} requests",
        received.len()
    );

    let results: Vec<Value> = serde_json::from_str(&result).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0]["error"].as_str(),
        Some("url_contains_secret"),
        "expected the D-19 secret gate to reject this URL before the D-16 \
         per-URL deadline could ever engage; got {:?}",
        results[0]
    );
}

// =============================================================================
// Phase 41.3 Plan 05 (D-08/D-17): config-ordered chain selection integration
// tests. All cases take the shared env_lock() and run under
// `--test-threads=1` (the whole binary already does, per the repo-wide
// convention this file follows).
//
// The target `url` argument these tests pass to `web_extract` is NOT fetched
// directly by the exa/firecrawl/tavily backends — they POST it as a JSON
// field to their own (overridden) endpoint. Per web_local::validate_url_async,
// a loopback host with IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK set is accepted by
// a pure string check with no network dial, so a throwaway loopback URL is
// safe to use as that target when the test doesn't also need `local` to GET
// it for real content.
// =============================================================================

/// Writes a `config.yaml` with a `tools.web_extract.chain` list to a fresh
/// tempdir and points `IRONHERMES_HOME` at it via `EnvGuard`, mirroring the
/// convention already used by `partial_batch_on_one_stuck_url` /
/// `stuck_url_error_entry_does_not_leak_the_request_url_query` (Plan 03).
fn write_chain_config(chain: &[&str]) -> (tempfile::TempDir, EnvGuard) {
    let cfg_tmp = tempfile::tempdir().expect("tempdir");
    let chain_yaml: String = chain.iter().map(|p| format!("      - {p}\n")).collect();
    let yaml = format!("tools:\n  web_extract:\n    chain:\n{chain_yaml}");
    std::fs::write(cfg_tmp.path().join("config.yaml"), yaml).expect("write test config.yaml");
    let home = EnvGuard::set(
        "IRONHERMES_HOME",
        cfg_tmp.path().to_str().expect("tmp path is valid utf8"),
    );
    (cfg_tmp, home)
}

/// Writes an empty `config.yaml` (no `tools.web_extract.chain` key at all) to
/// a fresh tempdir and points `IRONHERMES_HOME` at it — isolates the test from
/// the real operator config while leaving `tools.web_extract.chain` fully
/// unconfigured, so `ToolsConfig`'s serde default (the built-in Phase 25.2
/// D-04 order) applies exactly as it would for an operator who edits nothing.
fn write_isolated_home_no_chain_override() -> (tempfile::TempDir, EnvGuard) {
    let cfg_tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(cfg_tmp.path().join("config.yaml"), "").expect("write empty config.yaml");
    let home = EnvGuard::set(
        "IRONHERMES_HOME",
        cfg_tmp.path().to_str().expect("tmp path is valid utf8"),
    );
    (cfg_tmp, home)
}

/// `chain_order_promotes_exa_ahead_of_firecrawl`: with both `FIRECRAWL_API_KEY`
/// and `EXA_API_KEY` set and both endpoints mocked, a chain of
/// `["exa", "firecrawl", "local"]` results in the **exa** mock receiving the
/// request and the firecrawl mock receiving none — the inverse of the shipped
/// fixed order, proving configuration actually drives selection (D-17).
#[tokio::test(flavor = "multi_thread")]
async fn chain_order_promotes_exa_ahead_of_firecrawl() {
    let _g = env_lock().lock().await;
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");
    let _tk = EnvGuard::unset("TAVILY_API_KEY");

    let exa_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/exa_contents"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(
                    r#"{"results":[{"url":"http://target.example/x","title":"Exa Title","text":"Exa body text"}],"statuses":[]}"#,
                ),
        )
        .mount(&exa_server)
        .await;

    let firecrawl_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/firecrawl_scrape"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(
                    r#"{"success":true,"data":{"markdown":"Firecrawl body","metadata":{"title":"FC Title"}}}"#,
                ),
        )
        .mount(&firecrawl_server)
        .await;

    let _fk = EnvGuard::set("FIRECRAWL_API_KEY", "test-firecrawl-key");
    let _ek = EnvGuard::set("EXA_API_KEY", "test-exa-key");
    let _fe = EnvGuard::set(
        "FIRECRAWL_ENDPOINT_OVERRIDE",
        &format!("{}/firecrawl_scrape", firecrawl_server.uri()),
    );
    let _ee = EnvGuard::set(
        "EXA_ENDPOINT_OVERRIDE",
        &format!("{}/exa_contents", exa_server.uri()),
    );

    let (_cfg_tmp, _home) = write_chain_config(&["exa", "firecrawl", "local"]);

    let (reg, _counter, _tmp) = make_test_registry();
    let url = "http://127.0.0.1:1/target";
    let result = reg
        .handle_tool_call(
            "web_extract",
            json!({ "urls": [url], "use_llm_processing": false }),
        )
        .await
        .expect("web_extract call should succeed");

    let results: Vec<Value> = serde_json::from_str(&result).expect("parse Vec<ExtractionResult>");
    assert_eq!(results.len(), 1);
    assert!(
        results[0]["error"].is_null(),
        "expected success via exa (first in configured chain); got {:?}",
        results[0]
    );
    assert!(
        results[0]["content"]
            .as_str()
            .unwrap_or("")
            .contains("Exa body text"),
        "expected exa content; got {:?}",
        results[0]
    );

    let exa_received = exa_server.received_requests().await.unwrap();
    let firecrawl_received = firecrawl_server.received_requests().await.unwrap();
    assert!(
        !exa_received.is_empty(),
        "exa mock must receive the request — configured chain order promotes it \
         ahead of firecrawl"
    );
    assert_eq!(
        firecrawl_received.len(),
        0,
        "firecrawl mock must receive ZERO requests — exa (earlier in the \
         configured chain) already succeeded; got {} requests",
        firecrawl_received.len()
    );
}

/// `chain_can_demote_local_below_a_keyed_backend`: a chain of `["exa", "local"]`
/// with `EXA_API_KEY` set routes to exa; with `EXA_API_KEY` unset it falls
/// through to local. This is the concrete operator fix for the bot-wall
/// failure (blackbox run `0eaed980`), so it must be asserted, not assumed.
#[tokio::test(flavor = "multi_thread")]
async fn chain_can_demote_local_below_a_keyed_backend() {
    let _g = env_lock().lock().await;
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");
    let _fk = EnvGuard::unset("FIRECRAWL_API_KEY");
    let _tk = EnvGuard::unset("TAVILY_API_KEY");

    // ---- Sub-case 1: EXA_API_KEY set → exa wins over local ----
    {
        let exa_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/exa_contents"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        r#"{"results":[{"url":"http://target.example/x","title":"Exa Title","text":"Exa keyed body"}],"statuses":[]}"#,
                    ),
            )
            .mount(&exa_server)
            .await;
        let _ek = EnvGuard::set("EXA_API_KEY", "test-exa-key");
        let _ee = EnvGuard::set(
            "EXA_ENDPOINT_OVERRIDE",
            &format!("{}/exa_contents", exa_server.uri()),
        );

        let (_cfg_tmp, _home) = write_chain_config(&["exa", "local"]);
        let (reg, _counter, _tmp) = make_test_registry();
        let url = "http://127.0.0.1:1/target";
        let result = reg
            .handle_tool_call(
                "web_extract",
                json!({ "urls": [url], "use_llm_processing": false }),
            )
            .await
            .expect("web_extract call should succeed");
        let results: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert!(
            results[0]["content"]
                .as_str()
                .unwrap_or("")
                .contains("Exa keyed body"),
            "expected exa content when EXA_API_KEY is set; got {:?}",
            results[0]
        );
        let exa_received = exa_server.received_requests().await.unwrap();
        assert!(
            !exa_received.is_empty(),
            "exa mock must receive the request when keyed and ahead of local"
        );
    }

    // ---- Sub-case 2: EXA_API_KEY unset → falls through to local ----
    {
        let local_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/article"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html; charset=utf-8")
                    .set_body_raw(
                        r#"<html><head><title>Local Fallback</title></head><body><article><p>Local demoted-exa body.</p></article></body></html>"#,
                        "text/html",
                    ),
            )
            .mount(&local_server)
            .await;
        let _ek = EnvGuard::unset("EXA_API_KEY");

        let (_cfg_tmp, _home) = write_chain_config(&["exa", "local"]);
        let (reg, _counter, _tmp) = make_test_registry();
        let url = format!("{}/article", local_server.uri());
        let result = reg
            .handle_tool_call(
                "web_extract",
                json!({ "urls": [url], "use_llm_processing": false }),
            )
            .await
            .expect("web_extract call should succeed");
        let results: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert!(
            results[0]["content"]
                .as_str()
                .unwrap_or("")
                .contains("Local demoted-exa body"),
            "expected local fallback content when EXA_API_KEY is unset; got {:?}",
            results[0]
        );
    }
}

/// `unkeyed_chain_entry_is_skipped_not_failed`: a chain naming a provider
/// whose API key is absent skips to the next entry and still returns a
/// successful extraction. The would-be provider's endpoint is mocked to
/// return a 599 failure if ever hit, proving the entry is skipped BEFORE any
/// network attempt (not attempted-and-failed).
#[tokio::test(flavor = "multi_thread")]
async fn unkeyed_chain_entry_is_skipped_not_failed() {
    let _g = env_lock().lock().await;
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");
    let _fk = EnvGuard::unset("FIRECRAWL_API_KEY");
    let _ek = EnvGuard::unset("EXA_API_KEY");
    let _tk = EnvGuard::unset("TAVILY_API_KEY");

    let firecrawl_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/firecrawl_scrape"))
        .respond_with(ResponseTemplate::new(599))
        .mount(&firecrawl_server)
        .await;
    let _fe = EnvGuard::set(
        "FIRECRAWL_ENDPOINT_OVERRIDE",
        &format!("{}/firecrawl_scrape", firecrawl_server.uri()),
    );

    let local_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_raw(
                    r#"<html><head><title>T</title></head><body><article><p>Skip-then-succeed body.</p></article></body></html>"#,
                    "text/html",
                ),
        )
        .mount(&local_server)
        .await;

    let (_cfg_tmp, _home) = write_chain_config(&["firecrawl", "local"]);
    let (reg, _counter, _tmp) = make_test_registry();
    let url = format!("{}/article", local_server.uri());
    let result = reg
        .handle_tool_call(
            "web_extract",
            json!({ "urls": [url], "use_llm_processing": false }),
        )
        .await
        .expect("web_extract call should succeed");

    let results: Vec<Value> = serde_json::from_str(&result).expect("parse Vec<ExtractionResult>");
    assert_eq!(results.len(), 1);
    assert!(
        results[0]["error"].is_null(),
        "an unkeyed chain entry must be skipped, not fail the call; got {:?}",
        results[0]
    );
    assert!(
        results[0]["content"]
            .as_str()
            .unwrap_or("")
            .contains("Skip-then-succeed body"),
        "expected local content after skipping the unkeyed firecrawl entry; got {:?}",
        results[0]
    );

    let firecrawl_received = firecrawl_server.received_requests().await.unwrap();
    assert_eq!(
        firecrawl_received.len(),
        0,
        "unkeyed firecrawl entry must be skipped BEFORE any network attempt; got {} requests",
        firecrawl_received.len()
    );
}

/// `exhausted_chain_returns_err`: a chain of `["exa"]` with no key and no
/// other entry yields the pre-existing `Err` path, not a silent implicit
/// fall-through to local.
#[tokio::test(flavor = "multi_thread")]
async fn exhausted_chain_returns_err() {
    let _g = env_lock().lock().await;
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");
    let _fk = EnvGuard::unset("FIRECRAWL_API_KEY");
    let _ek = EnvGuard::unset("EXA_API_KEY");
    let _tk = EnvGuard::unset("TAVILY_API_KEY");

    let (_cfg_tmp, _home) = write_chain_config(&["exa"]);
    let (reg, _counter, _tmp) = make_test_registry();
    let url = "http://127.0.0.1:1/target";
    let result = reg
        .handle_tool_call(
            "web_extract",
            json!({ "urls": [url], "use_llm_processing": false }),
        )
        .await
        .expect("web_extract call itself succeeds — per-URL errors are D-02 partial-success, not a call-level Err");

    let results: Vec<Value> = serde_json::from_str(&result).expect("parse Vec<ExtractionResult>");
    assert_eq!(results.len(), 1);
    assert!(
        results[0]["error"].as_str().is_some(),
        "an exhausted chain (no keyed entry, no local fallback configured) must \
         surface a per-URL error, not silently fall through to local; got {:?}",
        results[0]
    );
    assert!(
        results[0]["content"].as_str().unwrap_or("").is_empty(),
        "no content expected on chain exhaustion; got {:?}",
        results[0]
    );
}

/// `default_chain_matches_pre_phase_behavior`: with no chain configured and
/// only `FIRECRAWL_API_KEY` set, firecrawl receives the request — the
/// Phase 25.2 D-04 behavior is unchanged for operators who edit nothing. This
/// is the back-compat guard for D-17's acknowledged risk.
#[tokio::test(flavor = "multi_thread")]
async fn default_chain_matches_pre_phase_behavior() {
    let _g = env_lock().lock().await;
    let _ssrf = EnvGuard::set("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK", "1");
    let _ek = EnvGuard::unset("EXA_API_KEY");
    let _tk = EnvGuard::unset("TAVILY_API_KEY");

    let firecrawl_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/firecrawl_scrape"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(
                    r#"{"success":true,"data":{"markdown":"Firecrawl default-order body","metadata":{"title":"FC"}}}"#,
                ),
        )
        .mount(&firecrawl_server)
        .await;

    let _fk = EnvGuard::set("FIRECRAWL_API_KEY", "test-firecrawl-key");
    let _fe = EnvGuard::set(
        "FIRECRAWL_ENDPOINT_OVERRIDE",
        &format!("{}/firecrawl_scrape", firecrawl_server.uri()),
    );

    // No tools.web_extract.chain key anywhere → ToolsConfig's serde default
    // (the built-in Phase 25.2 D-04 order) applies, exactly as it would for
    // an operator who upgrades without editing config.yaml.
    let (_cfg_tmp, _home) = write_isolated_home_no_chain_override();

    let (reg, _counter, _tmp) = make_test_registry();
    let url = "http://127.0.0.1:1/target";
    let result = reg
        .handle_tool_call(
            "web_extract",
            json!({ "urls": [url], "use_llm_processing": false }),
        )
        .await
        .expect("web_extract call should succeed");

    let results: Vec<Value> = serde_json::from_str(&result).expect("parse Vec<ExtractionResult>");
    assert!(
        results[0]["error"].is_null(),
        "expected firecrawl success via the unconfigured default chain; got {:?}",
        results[0]
    );
    assert!(
        results[0]["content"]
            .as_str()
            .unwrap_or("")
            .contains("Firecrawl default-order body"),
        "expected firecrawl content; got {:?}",
        results[0]
    );

    let firecrawl_received = firecrawl_server.received_requests().await.unwrap();
    assert!(
        !firecrawl_received.is_empty(),
        "firecrawl must receive the request under the unconfigured default \
         chain — Phase 25.2 D-04 behavior must be unchanged for operators who \
         edit nothing"
    );
}

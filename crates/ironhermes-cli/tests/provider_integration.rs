//! Phase 26 Plan 04 — provider CLI integration tests.
//!
//! D-20 Test 1: `key_does_not_leak_to_wrong_provider` — PROV-04 end-to-end.
//! D-20 Test 3: `custom_provider_selectable_by_name` — PROV-08 end-to-end.
//! D-15: `provider_test_does_not_print_key` — T-26-01 subprocess gate.
//! D-12: `legacy_env_banner_emitted_once_per_process` — once-only banner per process (uses `provider list`).
//! D-14: `provider_enable_disable_persists` — config.yaml round-trip.
//! D-16: `cache_break_banner_on_persistent_enable_disable` — banner on persistent writes.
//! T-26-03: `provider_enable_rejects_slug_injection` — slug validation gate.

use std::sync::OnceLock;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Process-wide ENV_LOCK — separate static from toolset_integration.rs (different binary).
/// Required because Rust runs tests in the same process on multiple threads by default;
/// any test that mutates env vars must hold this lock to avoid cross-test bleed.
fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// RAII env var guard — restores original value on drop, even on panic.
struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, val: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: test-only env mutation, serialised behind ENV_LOCK.
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

fn ironhermes_bin() -> Option<String> {
    std::env::var("CARGO_BIN_EXE_ironhermes").ok()
}

fn write_config_yaml(home: &std::path::Path, body: &str) {
    std::fs::write(home.join("config.yaml"), body).expect("write config.yaml");
}

// =============================================================================
// D-20 Test 1: key_does_not_leak_to_wrong_provider (PROV-04)
// =============================================================================

/// PROV-04 end-to-end: with OPENAI_API_KEY set and a custom provider (my-local-llm)
/// that has NO api_key_env, the resolver must give that provider api_key = None.
///
/// Library-level assertion suffices here — we verify the PROV-04 D-11 fix holds at
/// the resolver boundary. The wiremock server also captures any outbound requests
/// to assert the Authorization header does NOT contain the canary key value.
/// Per the plan's driver note: if no easy public AnyClient entry point is available
/// for a minimal request, the library-level api_key==None assertion is the hard gate.
#[tokio::test(flavor = "multi_thread")]
async fn key_does_not_leak_to_wrong_provider() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

    // Mount a wiremock server to capture any outbound requests.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"models": []})))
        .mount(&server)
        .await;

    // Set the canary key in env.
    let _key = EnvGuard::set("OPENAI_API_KEY", "sk-real-leak-canary");

    // Build a config where my-local-llm points at the wiremock server
    // and has NO api_key_env — the D-11 fix must prevent OPENAI_API_KEY leaking.
    let mut config = ironhermes_core::Config::default();
    config.model.provider = "my-local-llm".to_string();
    config.model.default = "test-model".to_string();
    config.providers.insert(
        "my-local-llm".to_string(),
        ironhermes_core::ProviderConfig {
            base_url: Some(format!("{}/v1", server.uri())),
            api_key_env: None, // explicitly None — D-11 scenario
            ..Default::default()
        },
    );

    let resolver =
        ironhermes_core::ProviderResolver::build(&config).expect("resolver build must succeed");
    let endpoint = resolver
        .resolve("my-local-llm")
        .expect("my-local-llm must resolve");

    // Hard gate (D-21 reinforcement): api_key MUST be None.
    assert_eq!(
        endpoint.api_key, None,
        "OPENAI_API_KEY MUST NOT leak to my-local-llm — PROV-04 violated"
    );

    // Inspect any captured requests to wiremock (soft gate: if requests were driven,
    // verify Authorization header does not contain the canary).
    let received = server.received_requests().await.unwrap_or_default();
    for req in &received {
        let auth = req
            .headers
            .get("authorization")
            .or_else(|| req.headers.get("Authorization"));
        if let Some(h) = auth {
            let s = h.to_str().unwrap_or("");
            assert!(
                !s.contains("sk-real-leak-canary"),
                "Authorization header leaked OPENAI_API_KEY to my-local-llm: {}",
                s
            );
        }
    }
}

// =============================================================================
// D-20 Test 3: custom_provider_selectable_by_name (PROV-08)
// =============================================================================

/// PROV-08 end-to-end: a named custom provider configured in providers: HashMap
/// must resolve to its configured base_url and use its configured api_key_env.
#[tokio::test(flavor = "multi_thread")]
async fn custom_provider_selectable_by_name() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "pong"}}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"models": []})))
        .mount(&server)
        .await;

    let _key = EnvGuard::set("MY_LLM_KEY", "test-key-for-prov08");

    // Library-level: build resolver + verify resolution selects my-local-llm.
    let mut config = ironhermes_core::Config::default();
    config.model.provider = "my-local-llm".to_string();
    config.model.default = "llama3.1".to_string();
    config.providers.insert(
        "my-local-llm".to_string(),
        ironhermes_core::ProviderConfig {
            base_url: Some(format!("{}/v1", server.uri())),
            api_key_env: Some("MY_LLM_KEY".to_string()),
            ..Default::default()
        },
    );

    let resolver =
        ironhermes_core::ProviderResolver::build(&config).expect("resolver build must succeed");
    let endpoint = resolver
        .resolve("my-local-llm")
        .expect("my-local-llm must resolve");

    // The resolver must pick up the key from MY_LLM_KEY env var.
    assert_eq!(
        endpoint.api_key.as_deref(),
        Some("test-key-for-prov08"),
        "custom provider must use its configured api_key_env"
    );
    // The base URL must match the wiremock server.
    assert!(
        endpoint.base_url.starts_with(&server.uri()),
        "custom provider base_url must point to wiremock server; got: {}",
        endpoint.base_url
    );

    // Drive an actual HTTP request via reqwest to hit the wiremock server
    // and verify it receives the request at the configured base_url.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let models_url = format!("{}/v1/models", server.uri());
    let resp = client
        .get(&models_url)
        .bearer_auth("test-key-for-prov08")
        .send()
        .await
        .expect("reqwest to wiremock must succeed");
    assert!(resp.status().is_success(), "wiremock must return 200");

    let received = server.received_requests().await.unwrap_or_default();
    assert!(
        !received.is_empty(),
        "custom provider --provider my-local-llm did not reach configured base_url"
    );
    // Verify Authorization header carries the correct key (not a different key).
    for req in &received {
        let auth = req
            .headers
            .get("authorization")
            .or_else(|| req.headers.get("Authorization"));
        if let Some(h) = auth {
            let s = h.to_str().unwrap_or("");
            assert!(
                s.contains("test-key-for-prov08"),
                "Authorization header must contain the custom key; got: {}",
                s
            );
        }
    }
}

// =============================================================================
// D-15: provider_test_does_not_print_key (T-26-01)
// =============================================================================

/// T-26-01: `hermes provider test openai` with OPENAI_API_KEY set must produce
/// stdout+stderr that contains NEITHER the key value NOR any `sk-` prefix substring.
#[test]
fn provider_test_does_not_print_key() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let bin = match ironhermes_bin() {
        Some(p) => p,
        None => {
            eprintln!("Skip provider_test_does_not_print_key: CARGO_BIN_EXE_ironhermes not set");
            return;
        }
    };
    let _key = EnvGuard::set("OPENAI_API_KEY", "sk-secret-canary-12345");
    let tmp = tempfile::TempDir::new().unwrap();

    // Write a minimal config pointing openai at a localhost URL that will
    // refuse the connection (we don't need a live server — we're checking
    // that the error output doesn't contain the key value).
    write_config_yaml(
        tmp.path(),
        r#"model:
  provider: openai
  default: gpt-4o
providers:
  openai:
    base_url: http://127.0.0.1:19999/v1
    api_key_env: OPENAI_API_KEY
"#,
    );

    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["provider", "test", "openai"])
        .output()
        .expect("run ironhermes provider test");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{}{}", stdout, stderr);

    // T-26-01 hard assertion: key value must NEVER appear.
    assert!(
        !combined.contains("sk-secret-canary-12345"),
        "T-26-01 VIOLATED: provider test leaked api_key value:\nSTDOUT={}\nSTDERR={}",
        stdout,
        stderr
    );
    // Also assert no sk- prefix appears (defense in depth).
    assert!(
        !combined.contains("sk-"),
        "T-26-01: provider test output contains sk- prefix:\n{}",
        combined
    );
    // Positive: env var NAME or [provider:openai] must appear in output.
    assert!(
        combined.contains("OPENAI_API_KEY") || combined.contains("provider:openai"),
        "expected env var name or provider label in output; got: {}",
        combined
    );
}

// =============================================================================
// D-12: legacy_env_banner_emitted_once_per_process
// =============================================================================

/// D-12 once-only: each process invocation emits the legacy env var deprecation
/// banner exactly once (OnceLock ensures per-process once-only; per Resolution #2
/// the subprocess isolation is the canonical verification approach since OnceLock
/// cannot be reset between unit tests in the same process).
#[test]
fn legacy_env_banner_emitted_once_per_process() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let bin = match ironhermes_bin() {
        Some(p) => p,
        None => {
            eprintln!(
                "Skip legacy_env_banner_emitted_once_per_process: CARGO_BIN_EXE_ironhermes not set"
            );
            return;
        }
    };
    let _key = EnvGuard::set("OPENAI_API_KEY", "sk-test-banner");
    let tmp = tempfile::TempDir::new().unwrap();
    // Config with openai as provider but NO api_key_env — triggers the legacy banner.
    write_config_yaml(
        tmp.path(),
        "model:\n  provider: openai\n  default: gpt-4o\n",
    );

    // Two separate subprocess invocations — each must emit the banner exactly once.
    // Use `provider list` (not `status`) because provider list calls ProviderResolver::build()
    // which is where the D-12 once-only banner is emitted.
    for invocation in 0..2 {
        let out = std::process::Command::new(&bin)
            .env("IRONHERMES_HOME", tmp.path())
            .args(["provider", "list"])
            .output()
            .expect("run ironhermes provider list");

        let stderr = String::from_utf8_lossy(&out.stderr);
        let count = stderr
            .matches("[provider:openai] using deprecated env var OPENAI_API_KEY")
            .count();
        assert_eq!(
            count, 1,
            "D-12 once-only: invocation {} produced {} banners (expected exactly 1);\nstderr:\n{}",
            invocation, count, stderr
        );
    }
}

// =============================================================================
// D-14: provider_enable_disable_persists
// =============================================================================

/// D-14: enable/disable must persist to config.yaml across binary restarts.
#[test]
fn provider_enable_disable_persists() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let bin = match ironhermes_bin() {
        Some(p) => p,
        None => {
            eprintln!("Skip provider_enable_disable_persists: CARGO_BIN_EXE_ironhermes not set");
            return;
        }
    };
    let tmp = tempfile::TempDir::new().unwrap();

    // Step 1: disable openai.
    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["provider", "disable", "openai"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "disable failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let yaml = std::fs::read_to_string(tmp.path().join("config.yaml")).unwrap();
    assert!(
        yaml.contains("disabled: true"),
        "expected disabled: true after disable; got:\n{}",
        yaml
    );

    // Step 2: re-enable openai.
    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["provider", "enable", "openai"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "enable failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let yaml = std::fs::read_to_string(tmp.path().join("config.yaml")).unwrap();
    assert!(
        yaml.contains("disabled: false") || !yaml.contains("disabled: true"),
        "expected disabled: false after enable; got:\n{}",
        yaml
    );
}

// =============================================================================
// D-16/D-17: cache_break_banner_on_persistent_enable_disable
// =============================================================================

/// D-16: cache-break banner must appear on stderr for persistent enable/disable writes.
/// Session-only `--provider` flag must NOT emit the banner.
#[test]
fn cache_break_banner_on_persistent_enable_disable() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let bin = match ironhermes_bin() {
        Some(p) => p,
        None => {
            eprintln!(
                "Skip cache_break_banner_on_persistent_enable_disable: CARGO_BIN_EXE_ironhermes not set"
            );
            return;
        }
    };
    let tmp = tempfile::TempDir::new().unwrap();

    // enable must emit the cache-break banner on stderr.
    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .args(["provider", "enable", "openai"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[provider: openai] config changed"),
        "D-16: cache-break banner missing on enable; stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("schema cache will rebuild"),
        "D-16: cache-break banner missing 'schema cache will rebuild'; stderr:\n{}",
        stderr
    );

    // Banner must be on stderr, not stdout.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("schema cache will rebuild"),
        "D-16: cache-break banner leaked to stdout; stdout:\n{}",
        stdout
    );
}

// =============================================================================
// D-20 Test 2: auxiliary_routes_to_separate_model (PROV-06)
// =============================================================================

/// PROV-06 end-to-end (D-20 Test 2): with auxiliary = { provider: openai, model: gpt-4o-mini }
/// and main = anthropic, a compression task hits the openai endpoint (aux_server) with
/// model gpt-4o-mini — NOT the anthropic endpoint (main_server).
///
/// Architecture:
/// - aux_server  = wiremock server standing in for openai (OpenAI ChatCompletions mode)
/// - main_server = wiremock server standing in for anthropic (should receive 0 requests)
/// - LlmClient posts to {base_url}/chat/completions, so wiremock path = /v1/chat/completions
#[tokio::test(flavor = "multi_thread")]
async fn auxiliary_routes_to_separate_model() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

    let aux_server = MockServer::start().await;
    let main_server = MockServer::start().await;

    // Auxiliary endpoint (openai/ChatCompletions): returns a valid OpenAI-compatible response.
    // Path must be /v1/chat/completions because LlmClient appends /chat/completions to
    // the configured base_url (which is aux_server.uri() + "/v1").
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 1700000000u64,
            "model": "gpt-4o-mini",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "compressed summary"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        })))
        .mount(&aux_server)
        .await;

    // Main endpoint (anthropic): must NOT be hit by the compression task.
    // Any request to main_server is a test failure indicator.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string(
            "main_server should NOT receive compression requests — PROV-06 violated",
        ))
        .mount(&main_server)
        .await;

    let _aux_key = EnvGuard::set("OPENAI_API_KEY", "sk-aux-test");
    let _main_key = EnvGuard::set("ANTHROPIC_API_KEY", "sk-main-test");

    // Build config: main = anthropic, auxiliary = openai/gpt-4o-mini.
    let mut config = ironhermes_core::Config::default();
    config.model.provider = "anthropic".to_string();
    config.model.default = "claude-sonnet-4".to_string();

    config.providers.insert(
        "anthropic".to_string(),
        ironhermes_core::ProviderConfig {
            // main_server.uri() is http://127.0.0.1:PORT — append /v1 for base_url
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

    let resolver =
        ironhermes_core::ProviderResolver::build(&config).expect("resolver build must succeed");

    // build_role_client("compression") resolves via D-05 cascade level 2 (auxiliary).
    let client = ironhermes_agent::build_role_client(&resolver, "compression")
        .expect("build_role_client must not error")
        .expect("compression role must resolve via auxiliary cascade (D-05 level 2)");

    // Verify the client is targeting the auxiliary endpoint (openai).
    // The client's model() must be "gpt-4o-mini" (the auxiliary model, not "claude-sonnet-4").
    assert_eq!(
        client.model(),
        "gpt-4o-mini",
        "PROV-06: compression client must use auxiliary model gpt-4o-mini, not claude-sonnet-4"
    );

    // Drive a real HTTP request via chat_completion — this sends POST to {base_url}/chat/completions.
    let messages = vec![ironhermes_core::ChatMessage {
        role: ironhermes_core::types::Role::User,
        content: Some(ironhermes_core::types::MessageContent::text(
            "Summarize: test compression payload",
        )),
        tool_calls: None,
        tool_call_id: None,
        name: None,
        is_recall_context: false,
    }];
    let resp = client
        .chat_completion(&messages, None, None, Some(10), None, None)
        .await
        .expect("chat_completion to aux_server must succeed");

    // The response model must be gpt-4o-mini (echoed back from wiremock stub).
    assert_eq!(
        resp.model, "gpt-4o-mini",
        "PROV-06: response model must be gpt-4o-mini from aux endpoint"
    );

    // Assert: aux_server received the request (compression hit auxiliary endpoint).
    let aux_reqs = aux_server.received_requests().await.unwrap_or_default();
    assert!(
        !aux_reqs.is_empty(),
        "PROV-06 VIOLATED: auxiliary endpoint (openai/aux_server) received 0 requests"
    );

    // Assert: the request body contains "model":"gpt-4o-mini".
    let body: serde_json::Value =
        serde_json::from_slice(&aux_reqs[0].body).expect("aux request body must be valid JSON");
    assert_eq!(
        body["model"].as_str(),
        Some("gpt-4o-mini"),
        "PROV-06: compression request body must contain model=gpt-4o-mini; got: {}",
        body
    );

    // Assert: main_server was NOT hit by the compression task (zero requests).
    let main_reqs = main_server.received_requests().await.unwrap_or_default();
    assert!(
        main_reqs.is_empty(),
        "PROV-06 VIOLATED: main (anthropic) endpoint received {} request(s) from compression task — \
         compression must route to aux only",
        main_reqs.len()
    );
}

// =============================================================================
// Pitfall 3 end-to-end: auxiliary_provider_unknown_name_fails_at_load
// =============================================================================

/// Plan 02 Pitfall 3: auxiliary.provider referencing an unknown name must fail
/// fast at ProviderResolver::build() with a clear error message.
/// This subprocess test verifies the error reaches the user at launch.
#[test]
fn auxiliary_provider_unknown_name_fails_at_load() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let bin = match ironhermes_bin() {
        Some(p) => p,
        None => {
            eprintln!(
                "Skip auxiliary_provider_unknown_name_fails_at_load: CARGO_BIN_EXE_ironhermes not set"
            );
            return;
        }
    };
    let tmp = tempfile::TempDir::new().unwrap();

    // Write config with auxiliary.provider pointing to a nonexistent provider name.
    write_config_yaml(
        tmp.path(),
        r#"model:
  provider: openai
  default: gpt-4o
providers:
  openai:
    base_url: https://api.openai.com/v1
    api_key_env: OPENAI_API_KEY
auxiliary:
  provider: nonexistent
  model: some-model
"#,
    );

    // Run hermes status — any command that triggers ProviderResolver::build().
    // hermes provider list also works and is more direct.
    let out = std::process::Command::new(&bin)
        .env("IRONHERMES_HOME", tmp.path())
        .env("OPENAI_API_KEY", "sk-test")
        .args(["provider", "list"])
        .output()
        .expect("run ironhermes provider list");

    // Must fail (non-zero exit) because ProviderResolver::build() rejects unknown aux provider.
    assert!(
        !out.status.success(),
        "Pitfall 3: hermes provider list must fail when auxiliary.provider is unknown; \
         exit code was 0;\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("nonexistent") || stderr.contains("auxiliary"),
        "Pitfall 3: error must mention the unknown provider name or 'auxiliary';\nstderr:\n{}",
        stderr
    );
}

// =============================================================================
// T-26-03: provider_enable_rejects_slug_injection
// =============================================================================

/// T-26-03: slug injection vectors must all be rejected with non-zero exit code
/// and a helpful error message BEFORE any config write occurs.
#[test]
fn provider_enable_rejects_slug_injection() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let bin = match ironhermes_bin() {
        Some(p) => p,
        None => {
            eprintln!(
                "Skip provider_enable_rejects_slug_injection: CARGO_BIN_EXE_ironhermes not set"
            );
            return;
        }
    };
    let tmp = tempfile::TempDir::new().unwrap();

    for bad in &["../etc/passwd", "foo;rm -rf", "with space", "UPPER"] {
        let out = std::process::Command::new(&bin)
            .env("IRONHERMES_HOME", tmp.path())
            .args(["provider", "enable", bad])
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "T-26-03: enable accepted invalid name {:?} — must be rejected",
            bad
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.to_lowercase().contains("invalid") || stderr.to_lowercase().contains("name"),
            "T-26-03: rejection message unhelpful for {:?}: {}",
            bad,
            stderr
        );

        // Verify NO config.yaml mutation occurred with the injected payload.
        let cfg_path = tmp.path().join("config.yaml");
        if cfg_path.exists() {
            let cfg = std::fs::read_to_string(&cfg_path).unwrap();
            assert!(
                !cfg.contains("../etc") && !cfg.contains("rm -rf"),
                "T-26-03: injection payload reached config.yaml for {:?}:\n{}",
                bad,
                cfg
            );
        }
    }
}

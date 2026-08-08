//! Phase 41.3 Plan 11 (D-18/D-19) — the wiremock zero-request proof.
//!
//! D-18/D-19's entire purpose is answering "if a provider is not configured,
//! is it ever called?". A test that only asserts `is_available() == false`
//! does NOT answer that — it re-states the implementation. The proof here is
//! a wiremock server that records requests, with an assertion that it
//! received ZERO requests when the credential is absent (Anti-Self-
//! Verification Guard, `41.3-VALIDATION.md`).
//!
//! Every test here mutates process environment (`EXA_API_KEY`) and takes the
//! shared `env_lock()` — run this file with `--test-threads=1` (crate-wide
//! convention for env-mutating tests).
//!
//! The fake tool is derived from the `Tool` trait directly rather than
//! reusing a real provider tool (`WebSearchTool`), so this proves the
//! MECHANISM — snapshot -> `is_available()` -> `get_definitions()` ->
//! `execute()` -> the wire — not one provider's wiring, and keeps working
//! when Plan 07 rewrites `web_search` on top of a multi-provider chain.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use ironhermes_core::{Config, ToolSchema};
use ironhermes_tools::credentials::ToolCredentials;
use ironhermes_tools::{Prerequisite, Tool, ToolRegistry};
use ironhermes_vault::SecretStore;
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

// =============================================================================
// env_lock — serialise tests that mutate EXA_API_KEY. tokio::sync::Mutex
// (not std::sync::Mutex): every test here is #[tokio::test] and holds the
// guard across ToolCredentials::resolve(...).await / execute(...).await —
// clippy::await_holding_lock (denied under this crate's -D warnings gate)
// forbids a std Mutex guard held across an await point. Mirrors
// tests/tool_credentials.rs's own env_lock() exactly.
// =============================================================================

fn env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn config_with_credential(key: &str, value: &str) -> Config {
    let mut config = Config::default();
    config
        .tools
        .credentials
        .insert(key.to_string(), value.to_string());
    config
}

// =============================================================================
// FakeCredentialGatedTool — a purpose-built fake, not a real provider tool.
// Mirrors WebSearchTool's actual shape (Plan 11 Task 1): a manual
// is_available() override reading the snapshot's has_credential() directly
// (not the default prereq-walking is_available()), a credentials() override
// so the tool is inspectable, and execute() that reads the resolved
// credential and issues exactly one HTTP request to a caller-supplied
// endpoint (the wiremock server's URI in these tests).
// =============================================================================

const PROBE_KEY: &str = "EXA_API_KEY";

struct FakeCredentialGatedTool {
    creds: Arc<ToolCredentials>,
    endpoint: String,
}

#[async_trait]
impl Tool for FakeCredentialGatedTool {
    fn name(&self) -> &str {
        "plan11_credential_gated_fake"
    }
    fn toolset(&self) -> &str {
        "test"
    }
    fn description(&self) -> &str {
        "Phase 41.3 Plan 11 gating-mechanism fixture — not a real provider."
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "plan11_credential_gated_fake",
            self.description(),
            json!({ "type": "object", "properties": {} }),
        )
    }
    fn is_available(&self) -> bool {
        self.creds.has_credential(PROBE_KEY)
    }
    fn prerequisites(&self) -> Vec<Prerequisite> {
        vec![Prerequisite {
            kind: "env_var".to_string(),
            name: PROBE_KEY.to_string(),
            description: "Plan 11 gating-mechanism probe credential.".to_string(),
            required: true,
            group: None,
        }]
    }
    fn credentials(&self) -> Option<&ToolCredentials> {
        Some(self.creds.as_ref())
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        let api_key = self
            .creds
            .credential(PROBE_KEY)
            .ok_or_else(|| anyhow::anyhow!("{PROBE_KEY} not set"))?;
        let client = reqwest::Client::new();
        let response = client
            .get(&self.endpoint)
            .bearer_auth(api_key.expose_secret())
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("request failed: {e}"))?;
        Ok(format!("status: {}", response.status()))
    }
}

/// D-18/D-19: with the credential absent from env, config, AND vault, the
/// fake tool must be absent from `get_definitions()`, and — the real proof —
/// the mock server must receive LITERALLY ZERO requests. A catch-all matcher
/// (`wiremock::matchers::any()`) with an expectation of exactly zero calls is
/// mounted BEFORE the registry is built; the `MockGuard` verifies on drop
/// (`.expect(0).mount_as_scoped(&server).await`), panicking the test if any
/// request — of any method, to any path — ever arrives.
#[tokio::test]
async fn an_unconfigured_tool_is_absent_from_the_schema_and_issues_no_request() {
    let _g = env_lock().lock().await;
    // SAFETY: env_lock held, --test-threads=1.
    unsafe { std::env::remove_var(PROBE_KEY) };

    let server = MockServer::start().await;
    // The mount expression that IS the proof: any request at all fails this test.
    let _guard = Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount_as_scoped(&server)
        .await;

    let creds = Arc::new(
        ToolCredentials::resolve(&Config::default(), None)
            .await
            .expect("resolve must succeed with no store"),
    );

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FakeCredentialGatedTool {
        creds: creds.clone(),
        endpoint: server.uri(),
    }));

    let schemas = registry.get_definitions(None);
    assert!(
        !schemas
            .iter()
            .any(|s| s.function.name == "plan11_credential_gated_fake"),
        "an unconfigured provider's tool must be absent from get_definitions(), got: {schemas:?}"
    );

    // `_guard` drops here (end of scope) — `MockGuard::drop` blocks on
    // verifying the `.expect(0)` expectation and panics if it was violated.
}

/// The positive control: without this half, the negative test above would
/// also pass against a fake tool that can never call anything (e.g. one with
/// a typo'd endpoint) — proving nothing. Here the credential is supplied via
/// the snapshot's CONFIG tier, so the tool must appear in `get_definitions()`
/// and dispatching it must make the mock record EXACTLY one request.
#[tokio::test]
async fn a_configured_tool_is_present_and_its_request_reaches_the_server() {
    let _g = env_lock().lock().await;
    // SAFETY: env_lock held, --test-threads=1.
    unsafe { std::env::remove_var(PROBE_KEY) };

    let server = MockServer::start().await;
    let _guard = Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let config = config_with_credential(PROBE_KEY, "plan11-config-tier-positive-control");
    let creds = Arc::new(
        ToolCredentials::resolve(&config, None)
            .await
            .expect("resolve must succeed with no store"),
    );

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FakeCredentialGatedTool {
        creds: creds.clone(),
        endpoint: server.uri(),
    }));

    let schemas = registry.get_definitions(None);
    assert!(
        schemas
            .iter()
            .any(|s| s.function.name == "plan11_credential_gated_fake"),
        "a configured provider's tool must be present in get_definitions(), got: {schemas:?}"
    );

    let result = registry
        .dispatch("plan11_credential_gated_fake", json!({}))
        .await;
    result.expect("dispatch must succeed once the credential is present");

    // `_guard` drops here — panics unless EXACTLY one request was received.
}

// =============================================================================
// MockStore — test-only SecretStore backend, mirrors tests/tool_credentials.rs
// =============================================================================

struct MockStore {
    value: SecretString,
}

#[async_trait]
impl SecretStore for MockStore {
    async fn get_secret(&self, key: &str) -> anyhow::Result<Option<SecretString>> {
        if key == PROBE_KEY {
            Ok(Some(self.value.clone()))
        } else {
            Ok(None)
        }
    }

    async fn put_secret(&self, _key: &str, _value: SecretString) -> anyhow::Result<()> {
        unimplemented!("this fixture only serves get_secret")
    }

    async fn delete_secret(&self, _key: &str) -> anyhow::Result<()> {
        unimplemented!("this fixture only serves get_secret")
    }

    async fn list_secrets(&self, _prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        unimplemented!("this fixture only serves get_secret")
    }
}

/// The third tier, end to end THROUGH THE REGISTRY — not only at the
/// snapshot's own API (which `tests/tool_credentials.rs` already covers).
/// A credential supplied ONLY by the vault must make the tool visible in
/// `get_definitions()` and reachable via `dispatch()`, exactly like the
/// config-tier positive control above.
#[tokio::test]
async fn a_credential_supplied_only_by_the_vault_makes_the_tool_visible() {
    let _g = env_lock().lock().await;
    // SAFETY: env_lock held, --test-threads=1.
    unsafe { std::env::remove_var(PROBE_KEY) };

    let server = MockServer::start().await;
    let _guard = Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let store = MockStore {
        value: SecretString::from("plan11-vault-tier-value".to_string()),
    };
    let creds = Arc::new(
        ToolCredentials::resolve(&Config::default(), Some(&store))
            .await
            .expect("resolve must succeed"),
    );

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FakeCredentialGatedTool {
        creds: creds.clone(),
        endpoint: server.uri(),
    }));

    let schemas = registry.get_definitions(None);
    assert!(
        schemas
            .iter()
            .any(|s| s.function.name == "plan11_credential_gated_fake"),
        "a tool with a vault-only credential must be present in get_definitions(), got: {schemas:?}"
    );

    let result = registry
        .dispatch("plan11_credential_gated_fake", json!({}))
        .await;
    result.expect("dispatch must succeed once the vault-tier credential resolves");

    // `_guard` drops here — panics unless EXACTLY one request was received.
}

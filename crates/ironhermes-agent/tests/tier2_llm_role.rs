//! D-16: Tier 2 consolidation runs on `model.roles.memory`, falling back to the main model, with
//! the config read from the profile home it was built for (no `IRONHERMES_HOME` involved).

use std::collections::HashMap;
use std::path::Path;

use ironhermes_agent::memory::factory::build_memory_manager_at;
use ironhermes_agent::memory::tier2_llm::ConfigBackedTier2Llm;
use ironhermes_core::ChatMessage;
use ironhermes_core::config::{
    Config, EmbeddingsConfig, MemoryConfig, ModelRoleConfig, ProviderConfig,
};
use ironhermes_state::StateStore;
use ironhermes_tier2::llm::Tier2LlmHandle;
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn mock(reply: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-test", "object": "chat.completion", "created": 0, "model": "m",
            "choices": [{"index": 0, "finish_reason": "stop",
                         "message": {"role": "assistant", "content": reply}}]
        })))
        .mount(&server)
        .await;
    server
}

fn provider(server: &MockServer) -> ProviderConfig {
    ProviderConfig {
        base_url: Some(server.uri()),
        api_key: Some("test-key".into()),
        // providers.<name>.default_model beats model.default (provider.rs overlay).
        default_model: Some("main-model".into()),
        ..Default::default()
    }
}

/// Main provider `main_p` (model `main-model`) at `main`; optional `memory` role at `role`.
fn config(main: &MockServer, role: Option<&MockServer>) -> Config {
    let mut cfg = Config::default();
    cfg.model.provider = "main_p".into();
    cfg.model.default = "main-model".into();
    let mut providers = HashMap::from([("main_p".to_string(), provider(main))]);
    if let Some(role) = role {
        providers.insert("role_p".into(), provider(role));
        cfg.model.roles.insert(
            "memory".into(),
            ModelRoleConfig {
                provider: "role_p".into(),
                model: Some("role-model".into()),
            },
        );
    }
    cfg.providers = providers;
    cfg
}

async fn bodies(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

#[tokio::test]
async fn tier2_llm_role_memory_routes_to_role_provider() {
    let (main, role) = (mock("from main").await, mock("from role").await);
    let llm = ConfigBackedTier2Llm::with_config(config(&main, Some(&role)));

    let out = llm.complete("sys".into(), "usr".into(), 100).await.unwrap();

    assert_eq!(out, "from role");
    let sent = bodies(&role).await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["model"], "role-model");
    assert_eq!(sent[0]["messages"][0]["role"], "system");
    assert_eq!(sent[0]["messages"][1]["content"], "usr");
    assert!(bodies(&main).await.is_empty(), "main must not be called");
}

#[tokio::test]
async fn tier2_llm_role_unset_falls_back_to_main() {
    let main = mock("from main").await;
    let llm = ConfigBackedTier2Llm::with_config(config(&main, None));

    let out = llm.complete("sys".into(), "usr".into(), 100).await.unwrap();

    assert_eq!(out, "from main");
    let sent = bodies(&main).await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["model"], "main-model");
}

fn write_config(home: &Path, cfg: &Config) {
    std::fs::write(
        home.join("config.yaml"),
        serde_yaml::to_string(cfg).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn tier2_llm_role_for_home_reads_that_homes_config() {
    let main = mock("from home").await;
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), &config(&main, None));
    let llm = ConfigBackedTier2Llm::for_home(home.path());

    assert_eq!(
        llm.complete("s".into(), "u".into(), 50).await.unwrap(),
        "from home"
    );
    assert_eq!(bodies(&main).await.len(), 1);

    // The file is re-read per call: an edit applies without rebuilding the handle.
    let other = mock("from edited").await;
    write_config(home.path(), &config(&other, None));
    assert_eq!(
        llm.complete("s".into(), "u".into(), 50).await.unwrap(),
        "from edited"
    );
    assert_eq!(bodies(&main).await.len(), 1);
}

#[tokio::test]
async fn tier2_llm_role_factory_attaches_llm() {
    let main = mock(r#"{"claims":[]}"#).await;
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), &config(&main, None));
    let mut state = StateStore::new(home.path().join("state.db")).unwrap();
    state
        .create_session("s1", "cli", None, None, None, None)
        .unwrap();
    state
        .add_message("s1", &ChatMessage::user("I like trains"))
        .unwrap();

    let cfg = MemoryConfig {
        tier2_enabled: true,
        consolidation_enabled: true,
        embeddings: EmbeddingsConfig {
            base_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let mgr = build_memory_manager_at(&cfg, home.path())
        .await
        .unwrap()
        .expect("memory enabled");
    let tier2 = mgr.lock().await.tier2().expect("tier2 attached");
    let report = tier2.consolidate().await.unwrap();

    assert_eq!(report.llm_calls, 1, "{report:?}");
    assert_eq!(bodies(&main).await.len(), 1);
}

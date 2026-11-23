//! D-12 / D-15 / D-16 end to end: `memory_consolidate` through the production registration seam,
//! a seeded `state.db`, and a mocked OpenAI-compatible memory model. Nothing here touches
//! `IRONHERMES_HOME`; embeddings point at a refused loopback port so search degrades to keyword.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use ironhermes_agent::MemoryManager;
use ironhermes_agent::memory::factory::build_memory_manager_at;
use ironhermes_core::ChatMessage;
use ironhermes_core::config::{Config, EmbeddingsConfig, MemoryConfig, ProviderConfig};
use ironhermes_state::StateStore;
use ironhermes_tools::ToolRegistry;
use ironhermes_tools::memory_tool::SharedMemoryManager;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Env {
    home: tempfile::TempDir,
    server: MockServer,
    reg: ToolRegistry,
    message_id: i64,
}

fn memory_cfg(consolidation: bool) -> MemoryConfig {
    MemoryConfig {
        tier2_enabled: true,
        consolidation_enabled: consolidation,
        embeddings: EmbeddingsConfig {
            base_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Home with a main model at the mock, one seeded user turn, and Tier 2 on.
async fn env(consolidation: bool) -> Env {
    let home = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;

    let mut cfg = Config::default();
    cfg.model.provider = "mock".into();
    cfg.model.default = "main-model".into();
    cfg.providers = HashMap::from([(
        "mock".to_string(),
        ProviderConfig {
            base_url: Some(server.uri()),
            api_key: Some("test-key".into()),
            default_model: Some("main-model".into()),
            ..Default::default()
        },
    )]);
    std::fs::write(
        home.path().join("config.yaml"),
        serde_yaml::to_string(&cfg).unwrap(),
    )
    .unwrap();

    let mut state = StateStore::new(home.path().join("state.db")).unwrap();
    state
        .create_session("s1", "cli", None, None, None, None)
        .unwrap();
    let message_id = state
        .add_message(
            "s1",
            &ChatMessage::user("Please keep replies concise on mobile"),
        )
        .unwrap();

    let mgr: Arc<Mutex<MemoryManager>> =
        build_memory_manager_at(&memory_cfg(consolidation), home.path())
            .await
            .unwrap()
            .expect("memory enabled");
    let mut reg = ToolRegistry::new();
    reg.register_memory_tool(mgr as SharedMemoryManager);
    Env {
        home,
        server,
        reg,
        message_id,
    }
}

impl Env {
    async fn mock_claim(&self) {
        let reply = json!({"claims": [{
            "text": "User prefers concise replies on mobile", "kind": "opinion",
            "subject": "user", "direct": true, "durable": true,
            "message_ids": [self.message_id]
        }]})
        .to_string();
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "c", "object": "chat.completion", "created": 0, "model": "m",
                "choices": [{"index": 0, "finish_reason": "stop",
                             "message": {"role": "assistant", "content": reply}}]
            })))
            .mount(&self.server)
            .await;
    }

    async fn call(&self, tool: &str, args: Value) -> Value {
        let raw = self.reg.dispatch(tool, args).await.unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    async fn llm_requests(&self) -> usize {
        self.server.received_requests().await.unwrap().len()
    }
}

fn home_file(home: &Path, rel: &str) -> String {
    std::fs::read_to_string(home.join("tier2").join(rel)).unwrap()
}

#[tokio::test]
async fn consolidate_tool_end_to_end() {
    let e = env(true).await;
    e.mock_claim().await;

    let first = e.call("memory_consolidate", json!({})).await;
    assert_eq!(first["claims_added"], 1, "{first}");
    assert_eq!(first["llm_calls"], 1, "{first}");
    assert_eq!(e.llm_requests().await, 1);

    let found = e.call("memory_search", json!({"query": "concise"})).await;
    let hit = &found["results"][0];
    assert!(
        hit["text"].as_str().unwrap().contains("concise replies"),
        "{found}"
    );
    let claim_id = hit["claim_id"].as_str().unwrap().to_string();

    let explained = e
        .call("memory_explain", json!({"claim_id": claim_id}))
        .await;
    let source = explained["evidence"][0]["source"].as_str().unwrap();
    assert!(source.starts_with("memory://live/"), "{explained}");

    assert!(
        home_file(e.home.path(), "bank/opinions.md").contains(&claim_id),
        "bank file must cite the claim"
    );

    let second = e.call("memory_consolidate", json!({})).await;
    assert_eq!(second["llm_calls"], 0, "{second}");
    assert_eq!(e.llm_requests().await, 1, "second call made an LLM request");
}

#[tokio::test]
async fn consolidate_tool_disabled_reports_reason() {
    let e = env(false).await;
    e.mock_claim().await;

    let out = e.call("memory_consolidate", json!({})).await;

    let skipped = out["skipped"].as_str().expect("skipped reason");
    assert!(skipped.contains("memory.consolidation_enabled"), "{out}");
    assert_eq!(out["claims_added"], 0);
    assert_eq!(e.llm_requests().await, 0);
}

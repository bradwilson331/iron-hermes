//! D-12 / D-21 / D-23 end to end: `memory_forget` (stage | cancel) through the production
//! registration seam. Nothing here touches `IRONHERMES_HOME`; embeddings point at a refused
//! loopback port so search degrades to keyword and no model is downloaded.

use std::path::Path;
use std::sync::Arc;

use ironhermes_agent::MemoryManager;
use ironhermes_agent::memory::factory::build_memory_manager_at;
use ironhermes_core::config::{EmbeddingsConfig, MemoryConfig};
use ironhermes_tools::ToolRegistry;
use ironhermes_tools::memory_tool::SharedMemoryManager;
use serde_json::{Value, json};
use tokio::sync::Mutex;

const FACT: &str = "User prefers concise Telegram replies";

fn memory_cfg(forgetting: bool) -> MemoryConfig {
    MemoryConfig {
        tier2_enabled: true,
        forgetting_enabled: forgetting,
        embeddings: EmbeddingsConfig {
            base_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

async fn registry(home: &Path, forgetting: bool) -> ToolRegistry {
    let mgr: Arc<Mutex<MemoryManager>> = build_memory_manager_at(&memory_cfg(forgetting), home)
        .await
        .unwrap()
        .expect("memory enabled");
    let mut reg = ToolRegistry::new();
    reg.register_memory_tool(mgr as SharedMemoryManager);
    reg
}

async fn call(reg: &ToolRegistry, tool: &str, args: Value) -> anyhow::Result<Value> {
    let raw = reg.dispatch(tool, args).await?;
    Ok(serde_json::from_str(&raw).unwrap())
}

/// Add FACT to Tier 1 (which indexes it into Tier 2) and return its claim id.
async fn seed(reg: &ToolRegistry) -> String {
    reg.dispatch(
        "memory",
        json!({"action": "add", "target": "memory", "content": FACT}),
    )
    .await
    .unwrap();
    find(reg).await.expect("the added fact is searchable")
}

async fn find(reg: &ToolRegistry) -> Option<String> {
    let r = call(reg, "memory_search", json!({"query": "telegram"}))
        .await
        .unwrap();
    r["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["text"] == FACT)
        .map(|h| h["claim_id"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn forget_tool_end_to_end() {
    let home = tempfile::tempdir().unwrap();
    let reg = registry(home.path(), true).await;
    let x = seed(&reg).await;

    let staged = call(&reg, "memory_forget", json!({"claim_id": x}))
        .await
        .unwrap();
    assert_eq!(staged["status"], "staged");
    assert_eq!(staged["claim_id"], x.as_str());
    assert!(find(&reg).await.is_none(), "hidden immediately");
    assert!(
        call(&reg, "memory_explain", json!({"claim_id": x}))
            .await
            .unwrap()
            .get("error")
            .is_some(),
        "explain no longer finds it"
    );
    let pending = std::fs::read_to_string(home.path().join("tier2/forget/pending.json")).unwrap();
    assert!(pending.contains(&x), "{pending}");

    let cancelled = call(
        &reg,
        "memory_forget",
        json!({"claim_id": x, "action": "cancel"}),
    )
    .await
    .unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(find(&reg).await.as_deref(), Some(x.as_str()));

    let audit = std::fs::read_to_string(home.path().join("logs/audit.jsonl")).unwrap();
    assert!(audit.contains("memory_forget_staged") && audit.contains("memory_forget_unstaged"));
}

#[tokio::test]
async fn forget_tool_flag_off_errors() {
    let home = tempfile::tempdir().unwrap();
    let reg = registry(home.path(), false).await;
    let x = seed(&reg).await;

    let err = call(&reg, "memory_forget", json!({"claim_id": x}))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("memory.forgetting_enabled"),
        "{err}"
    );
    assert_eq!(find(&reg).await.as_deref(), Some(x.as_str()), "unchanged");
    assert!(!home.path().join("tier2/forget/pending.json").exists());
}

//! Phase 58.2 Plan 18 (D-31): the agent's `memory_note` tool writes a dated daily note to the live
//! log, `memory_search` finds it with a line citation, and it never enters the prompt. Every home
//! is an explicit tempdir; embeddings point at a refused loopback port so no test starts a model
//! download (search degrades to keyword).

use std::path::Path;
use std::sync::Arc;

use ironhermes_agent::MemoryManager;
use ironhermes_agent::memory::factory::build_memory_manager_at;
use ironhermes_agent::prompt_builder::PromptBuilder;
use ironhermes_core::config::{EmbeddingsConfig, MemoryConfig};
use ironhermes_core::tier2::is_valid_claim_id;
use ironhermes_tools::ToolRegistry;
use ironhermes_tools::memory_tool::SharedMemoryManager;
use serde_json::{Value, json};
use tokio::sync::Mutex;

const NOTE: &str = "Met Dana at the harbour cafe about the kiln repair";

fn cfg(tier2: bool) -> MemoryConfig {
    MemoryConfig {
        tier2_enabled: tier2,
        provider: "file".into(),
        embeddings: EmbeddingsConfig {
            base_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

async fn manager(home: &Path, tier2: bool) -> Arc<Mutex<MemoryManager>> {
    build_memory_manager_at(&cfg(tier2), home)
        .await
        .unwrap()
        .expect("memory is enabled")
}

fn registry(mgr: Arc<Mutex<MemoryManager>>) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.register_memory_tool(mgr as SharedMemoryManager);
    reg
}

#[tokio::test]
async fn note_write_is_found_by_memory_search() {
    let home = tempfile::tempdir().unwrap();
    let mgr = manager(home.path(), true).await;
    let reg = registry(mgr.clone());

    let out: Value = serde_json::from_str(
        &reg.dispatch("memory_note", json!({"content": NOTE}))
            .await
            .unwrap(),
    )
    .unwrap();
    let id = out["claim_id"].as_str().unwrap();
    let source = out["source"].as_str().unwrap();
    assert!(is_valid_claim_id(id), "{id}");
    assert!(source.starts_with("memory://live/"), "{source}");

    let found: Value = serde_json::from_str(
        &reg.dispatch("memory_search", json!({"query": "kiln"}))
            .await
            .unwrap(),
    )
    .unwrap();
    let hits = found["results"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{found}");
    assert_eq!(hits[0]["claim_id"], id);
    assert_eq!(hits[0]["source"], source);

    // The citation names the note's own line: `memory://live/<date>.md#L<n>`.
    let (file, line) = source
        .strip_prefix("memory://live/")
        .and_then(|s| s.split_once("#L"))
        .unwrap();
    let n: usize = line.parse().unwrap();
    let body = std::fs::read_to_string(home.path().join("tier2/live").join(file)).unwrap();
    assert_eq!(body.lines().nth(n - 1).unwrap(), format!("- [{id}] {NOTE}"));

    // Never in Tier 1 and never in a new session's prompt.
    for f in ["MEMORY.md", "USER.md"] {
        let p = home.path().join("memories").join(f);
        let text = std::fs::read_to_string(p).unwrap_or_default();
        assert!(!text.contains("kiln"), "{f} must not hold the note");
    }
    let mut b = PromptBuilder::new("m", "cli").with_session_id("s-note");
    b.set_memory_manager(mgr.clone());
    b.load_memory().await;
    let (durable, ephemeral) = b.build_split();
    assert!(!durable.contains("kiln") && !ephemeral.contains("kiln"));
}

#[tokio::test]
async fn note_tool_only_with_tier2() {
    let off = tempfile::tempdir().unwrap();
    let on = tempfile::tempdir().unwrap();
    let off_reg = registry(manager(off.path(), false).await);
    let on_reg = registry(manager(on.path(), true).await);
    let names = |r: &ToolRegistry| -> Vec<String> {
        r.list_tools().into_iter().map(str::to_string).collect()
    };
    assert!(!names(&off_reg).contains(&"memory_note".to_string()));
    assert!(names(&on_reg).contains(&"memory_note".to_string()));
}

//! Phase 58.2 Plan 01 tracer: a Tier 1 `memory` add is indexed into the per-profile Tier 2 store
//! (D-03) and found by the `memory_search` tool (D-12), dispatched through a `ToolRegistry`.
//!
//! Every home is an explicit tempdir; nothing here touches `IRONHERMES_HOME`.

use std::path::Path;
use std::sync::Arc;

use ironhermes_agent::MemoryManager;
use ironhermes_agent::memory::SharedProvider;
use ironhermes_agent::memory::factory::build_memory_manager_at;
use ironhermes_core::config::{EmbeddingsConfig, MemoryConfig};
use ironhermes_core::memory_provider::MemoryProvider;
use ironhermes_core::tier2::{
    Tier1SyncReport, Tier2Handle, Tier2SearchMode, Tier2SearchResponse, is_valid_claim_id,
};
use ironhermes_core::{MemoryEntries, MemoryStore};
use ironhermes_tools::ToolRegistry;
use ironhermes_tools::memory_tool::SharedMemoryManager;
use tokio::sync::Mutex;

const FACT: &str = "User prefers concise Telegram replies";

/// Tier 1 file provider at an explicit home, built the way the factory's
/// `build_tokio_file_provider` does.
async fn tier1_provider(home: &Path) -> SharedProvider {
    let mut store = MemoryStore::new(home.join("memories"));
    MemoryProvider::initialize(&mut store, "tracer", home, &serde_json::Value::Null)
        .await
        .unwrap();
    store.load_from_disk().unwrap();
    Arc::new(Mutex::new(store))
}

async fn manager_without_tier2(home: &Path) -> Arc<Mutex<MemoryManager>> {
    Arc::new(Mutex::new(
        MemoryManager::new(tier1_provider(home).await, None)
            .await
            .unwrap(),
    ))
}

/// Registry built the production way: `register_memory_tool` alone (D-12).
fn registry(mgr: Arc<Mutex<MemoryManager>>) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.register_memory_tool(mgr as SharedMemoryManager);
    reg
}

/// Tier 2 on, pointed at a refused loopback port so CI never starts a built-in model download.
fn tier2_cfg() -> MemoryConfig {
    MemoryConfig {
        tier2_enabled: true,
        embeddings: EmbeddingsConfig {
            base_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn add_args(content: &str) -> serde_json::Value {
    serde_json::json!({"action": "add", "target": "memory", "content": content})
}

async fn search(reg: &ToolRegistry, query: &str) -> serde_json::Value {
    let raw = reg
        .dispatch("memory_search", serde_json::json!({ "query": query }))
        .await
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[tokio::test]
async fn tracer_tier1_add_is_found_by_memory_search() {
    let home = tempfile::tempdir().unwrap();
    let mgr = build_memory_manager_at(&tier2_cfg(), home.path())
        .await
        .unwrap()
        .expect("memory is enabled");
    {
        let m = mgr.lock().await;
        assert!(m.tier2().is_some(), "boot must attach the handle");
        assert!(
            m.mirror_handle().is_none(),
            "Tier 2 is never the mirror (D-10)"
        );
    }

    let reg = registry(mgr);
    let names = reg.list_tools();
    assert!(names.contains(&"memory") && names.contains(&"memory_search"));

    // The `memory` output is identical to a manager with no Tier 2 attached (D-03).
    let plain_home = tempfile::tempdir().unwrap();
    let plain = registry(manager_without_tier2(plain_home.path()).await);
    let with_tier2 = reg.dispatch("memory", add_args(FACT)).await.unwrap();
    let without = plain.dispatch("memory", add_args(FACT)).await.unwrap();
    assert_eq!(with_tier2, without);

    let v = search(&reg, "Telegram").await;
    let first = &v["results"][0];
    assert_eq!(first["text"], FACT);
    let id = first["claim_id"].as_str().unwrap();
    assert!(is_valid_claim_id(id), "bad claim id {id}");
    assert!(
        first["source"]
            .as_str()
            .unwrap()
            .starts_with("memory://tier1/MEMORY.md#L")
    );
    assert_eq!(v["mode"], "keyword");
    assert!(
        !v["note"].as_str().unwrap().is_empty(),
        "D-07: note names the fallback"
    );

    // Tier 2 lives under <home>/tier2/ and nowhere else.
    assert!(home.path().join("tier2/tier2.db").is_file());
    assert!(!home.path().join("memory.db").exists());
}

/// D-29: a removed Tier 1 entry moves into today's live log and stays findable under its id.
#[tokio::test]
async fn tracer_tier1_remove_moves_claim_to_daily_log() {
    let home = tempfile::tempdir().unwrap();
    let mgr = build_memory_manager_at(&tier2_cfg(), home.path())
        .await
        .unwrap()
        .expect("memory is enabled");
    let reg = registry(mgr);

    reg.dispatch("memory", add_args(FACT)).await.unwrap();
    let before = search(&reg, "Telegram").await;
    assert_eq!(before["results"].as_array().unwrap().len(), 1);
    let id = before["results"][0]["claim_id"]
        .as_str()
        .unwrap()
        .to_string();

    reg.dispatch(
        "memory",
        serde_json::json!({"action": "remove", "target": "memory", "old_text": "concise Telegram"}),
    )
    .await
    .unwrap();

    let after = search(&reg, "Telegram").await;
    let hits = after["results"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "the moved claim stays findable");
    assert_eq!(hits[0]["claim_id"], id.as_str());
    let source = hits[0]["source"].as_str().unwrap();
    let rest = source
        .strip_prefix("memory://live/")
        .unwrap_or_else(|| panic!("not a live-log source: {source}"));
    let (file, line) = rest.split_once("#L").expect("source carries a line");
    let n: usize = line.parse().unwrap();
    let log = std::fs::read_to_string(home.path().join("tier2/live").join(file)).unwrap();
    assert_eq!(
        log.lines().nth(n - 1).unwrap(),
        format!("- [{id}] {FACT}"),
        "the cited line holds the moved text"
    );

    let md = std::fs::read_to_string(home.path().join("memories/MEMORY.md")).unwrap();
    assert!(!md.contains("Telegram"), "the entry left MEMORY.md");

    let raw = reg
        .dispatch("memory_explain", serde_json::json!({ "claim_id": id }))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let sources: Vec<&str> = v["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["source"].as_str().unwrap())
        .collect();
    assert!(
        sources
            .iter()
            .any(|s| s.starts_with("memory://tier1/MEMORY.md#L"))
    );
    assert!(sources.iter().any(|s| s.starts_with("memory://live/")));
}

/// Tier 2 handle whose sync always fails.
struct FailingSync;

#[async_trait::async_trait]
impl Tier2Handle for FailingSync {
    async fn search(&self, _q: &str, _l: usize) -> anyhow::Result<Tier2SearchResponse> {
        Ok(Tier2SearchResponse {
            results: vec![],
            mode: Tier2SearchMode::Keyword,
            note: None,
        })
    }
    async fn sync_tier1(&self, _e: &MemoryEntries) -> anyhow::Result<Tier1SyncReport> {
        anyhow::bail!("tier2 is down")
    }
    async fn explain(
        &self,
        _id: &str,
    ) -> anyhow::Result<Option<ironhermes_core::tier2::Tier2Explanation>> {
        Ok(None)
    }
    async fn list_recent(
        &self,
        _l: usize,
    ) -> anyhow::Result<Vec<ironhermes_core::tier2::Tier2Hit>> {
        Ok(vec![])
    }
    async fn consolidate(&self) -> anyhow::Result<ironhermes_core::tier2::ConsolidationReport> {
        Ok(Default::default())
    }
    async fn run_dreams(&self) -> anyhow::Result<ironhermes_core::tier2::DreamsReport> {
        Ok(Default::default())
    }
    async fn synthesis_snapshot(&self, _s: Option<&str>) -> Option<String> {
        None
    }
    async fn stage_forget(
        &self,
        _id: &str,
        _r: Option<&str>,
    ) -> anyhow::Result<ironhermes_core::tier2::ForgetOutcome> {
        anyhow::bail!("unused")
    }
    async fn unstage_forget(
        &self,
        _id: &str,
    ) -> anyhow::Result<ironhermes_core::tier2::ForgetOutcome> {
        anyhow::bail!("unused")
    }
    async fn list_staged(&self) -> anyhow::Result<Vec<ironhermes_core::tier2::Tier2Hit>> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn tracer_sync_failure_returns_tier1_result_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let mut mgr = MemoryManager::new(tier1_provider(home.path()).await, None)
        .await
        .unwrap();
    mgr.set_tier2(Arc::new(FailingSync));
    let reg = registry(Arc::new(Mutex::new(mgr)));

    let plain_home = tempfile::tempdir().unwrap();
    let plain = registry(manager_without_tier2(plain_home.path()).await);

    let got = reg.dispatch("memory", add_args(FACT)).await.unwrap();
    let want = plain.dispatch("memory", add_args(FACT)).await.unwrap();
    assert_eq!(
        got, want,
        "a failing Tier 2 sync must not change the Tier 1 result (T-58.2-04)"
    );

    let md = std::fs::read_to_string(home.path().join("memories/MEMORY.md")).unwrap();
    assert!(md.contains(FACT), "the entry must still be in MEMORY.md");
}

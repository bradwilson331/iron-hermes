//! Phase 58.2 Plan 04: D-03 boot-time indexing of existing Tier 1 entries, Tier 1 byte-identity
//! with Tier 2 on vs off, tombstone-honouring sync, and D-10 provider independence.
//!
//! Every home is an explicit tempdir; nothing here touches `IRONHERMES_HOME`. Embeddings point at
//! a refused loopback port so no test starts a model download (search degrades to keyword).

use std::path::Path;
use std::sync::Arc;

use ironhermes_agent::MemoryManager;
use ironhermes_agent::memory::factory::build_memory_manager_at;
use ironhermes_core::config::{EmbeddingsConfig, MemoryConfig};
use ironhermes_core::memory_store::MemoryTarget;
use ironhermes_tier2::store::{Store, text_hash};
use ironhermes_tools::ToolRegistry;
use ironhermes_tools::memory_tool::SharedMemoryManager;
use serde_json::{Value, json};
use tokio::sync::Mutex;

fn cfg(tier2: bool, provider: &str) -> MemoryConfig {
    MemoryConfig {
        tier2_enabled: tier2,
        provider: provider.into(),
        embeddings: EmbeddingsConfig {
            base_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

async fn build(home: &Path, tier2: bool, provider: &str) -> Arc<Mutex<MemoryManager>> {
    build_memory_manager_at(&cfg(tier2, provider), home)
        .await
        .unwrap()
        .expect("memory is enabled")
}

fn registry(mgr: Arc<Mutex<MemoryManager>>) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.register_memory_tool(mgr as SharedMemoryManager);
    reg
}

async fn mem(reg: &ToolRegistry, args: Value) -> String {
    reg.dispatch("memory", args).await.unwrap()
}

fn add(target: &str, content: &str) -> Value {
    json!({"action": "add", "target": target, "content": content})
}

async fn search(reg: &ToolRegistry, query: &str) -> Value {
    let raw = reg
        .dispatch("memory_search", json!({ "query": query }))
        .await
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn hit_count(v: &Value) -> usize {
    v["results"].as_array().unwrap().len()
}

/// Two MEMORY.md entries and one USER.md entry written through a Tier-2-off manager.
async fn seed_tier1(home: &Path) {
    let reg = registry(build(home, false, "file").await);
    mem(&reg, add("memory", "Project uses Rust edition 2024")).await;
    mem(
        &reg,
        add("memory", "Deploys happen from the develop branch"),
    )
    .await;
    mem(&reg, add("user", "Prefers concise Telegram replies")).await;
}

fn tier1_claims(home: &Path) -> usize {
    Store::open(&home.join("tier2/tier2.db"))
        .unwrap()
        .tier1_claims()
        .unwrap()
        .len()
}

#[tokio::test]
async fn tier1_observer_boot_indexes_existing_entries() {
    let home = tempfile::tempdir().unwrap();
    seed_tier1(home.path()).await;
    assert!(
        !home.path().join("tier2").exists(),
        "Tier 2 off writes nothing"
    );

    let reg = registry(build(home.path(), true, "file").await);
    assert_eq!(hit_count(&search(&reg, "Rust").await), 1);
    assert_eq!(hit_count(&search(&reg, "develop").await), 1);
    let user = search(&reg, "Telegram").await;
    assert_eq!(hit_count(&user), 1);
    assert_eq!(user["results"][0]["source"], "memory://tier1/USER.md#L1");
    assert_eq!(tier1_claims(home.path()), 3);

    // A second boot reconciles instead of duplicating.
    drop(reg);
    let reg = registry(build(home.path(), true, "file").await);
    assert_eq!(hit_count(&search(&reg, "Rust").await), 1);
    assert_eq!(tier1_claims(home.path()), 3);
}

#[tokio::test]
async fn tier1_observer_outputs_byte_identical() {
    let on_home = tempfile::tempdir().unwrap();
    let off_home = tempfile::tempdir().unwrap();
    let on_mgr = build(on_home.path(), true, "file").await;
    let off_mgr = build(off_home.path(), false, "file").await;
    let (on, off) = (registry(on_mgr.clone()), registry(off_mgr.clone()));

    let steps = [
        add("memory", "alpha fact about the build"),
        add("memory", "beta fact about deploys"),
        add("user", "gamma likes terse answers"),
        json!({"action": "replace", "target": "memory", "old_text": "alpha fact", "content": "alpha fact, revised"}),
        json!({"action": "remove", "target": "memory", "old_text": "beta fact"}),
    ];
    for step in steps {
        assert_eq!(
            mem(&on, step.clone()).await,
            mem(&off, step.clone()).await,
            "tool result diverged for {step}"
        );
    }

    let read = |h: &Path| std::fs::read(h.join("memories/MEMORY.md")).unwrap();
    assert_eq!(read(on_home.path()), read(off_home.path()));
    let user = |h: &Path| std::fs::read(h.join("memories/USER.md")).unwrap();
    assert_eq!(user(on_home.path()), user(off_home.path()));
    for t in [MemoryTarget::Memory, MemoryTarget::User] {
        assert_eq!(
            on_mgr.lock().await.format_for_system_prompt(t).await,
            off_mgr.lock().await.format_for_system_prompt(t).await,
        );
    }
}

/// Lines across `<home>/tier2/live/*.md` that carry `[<id>]`.
fn live_lines_with(home: &Path, id: &str) -> usize {
    let marker = format!("[{id}]");
    let Ok(dir) = std::fs::read_dir(home.join("tier2/live")) else {
        return 0;
    };
    dir.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .map(|s| s.lines().filter(|l| l.contains(&marker)).count())
        .sum()
}

#[tokio::test]
async fn tier1_observer_replace_moves_old_text() {
    let home = tempfile::tempdir().unwrap();
    let reg = registry(build(home.path(), true, "file").await);
    mem(&reg, add("memory", "The mascot is a heron")).await;
    assert_eq!(hit_count(&search(&reg, "heron").await), 1);
    mem(
        &reg,
        json!({"action": "replace", "target": "memory", "old_text": "heron", "content": "The mascot is an otter"}),
    )
    .await;
    let old = search(&reg, "heron").await;
    assert_eq!(hit_count(&old), 1, "the replaced text moved, not erased");
    assert!(
        old["results"][0]["source"]
            .as_str()
            .unwrap()
            .starts_with("memory://live/")
    );
    let new = search(&reg, "otter").await;
    assert_eq!(hit_count(&new), 1);
    assert_eq!(new["results"][0]["source"], "memory://tier1/MEMORY.md#L1");
}

#[tokio::test]
async fn tier1_observer_moved_entry_survives_restart() {
    let home = tempfile::tempdir().unwrap();
    let reg = registry(build(home.path(), true, "file").await);
    mem(&reg, add("memory", "The mascot is a heron")).await;
    mem(
        &reg,
        json!({"action": "remove", "target": "memory", "old_text": "heron"}),
    )
    .await;
    let first = search(&reg, "heron").await;
    assert_eq!(hit_count(&first), 1);
    let id = first["results"][0]["claim_id"]
        .as_str()
        .unwrap()
        .to_string();
    drop(reg);

    // Rebuilding runs the boot sync on the same home.
    let reg = registry(build(home.path(), true, "file").await);
    let again = search(&reg, "heron").await;
    assert_eq!(hit_count(&again), 1);
    assert_eq!(again["results"][0]["claim_id"], id.as_str());
    assert_eq!(tier1_claims(home.path()), 1);
    assert_eq!(live_lines_with(home.path(), &id), 1);
}

#[tokio::test]
async fn tier1_observer_honors_tombstones() {
    let home = tempfile::tempdir().unwrap();
    seed_tier1(home.path()).await;
    std::fs::create_dir_all(home.path().join("tier2")).unwrap();
    Store::open(&home.path().join("tier2/tier2.db"))
        .unwrap()
        .insert_tombstone(&text_hash("Deploys happen from the develop branch"))
        .unwrap();

    let mgr = build(home.path(), true, "file").await;
    let reg = registry(mgr.clone());
    assert_eq!(hit_count(&search(&reg, "develop").await), 0);
    assert_eq!(hit_count(&search(&reg, "Rust").await), 1);

    let (handle, entries) = {
        let m = mgr.lock().await;
        (m.tier2().unwrap(), m.to_memory_entries().await)
    };
    let report = handle.sync_tier1(&entries).await.unwrap();
    assert_eq!(report.skipped_tombstoned, 1);
    assert_eq!(report.added, 0);
}

/// Seed Tier 1 through `provider` with Tier 2 off, then switch Tier 2 on: the entry is searchable
/// and Tier 2 is never the mirror. Returns the Tier-2-on registry.
async fn assert_provider_works(home: &Path, provider: &str) -> ToolRegistry {
    let off = registry(build(home, false, provider).await);
    mem(&off, add("memory", "Provider independent kiwi fact")).await;
    drop(off);

    let mgr = build(home, true, provider).await;
    assert!(mgr.lock().await.mirror_handle().is_none());
    let reg = registry(mgr);
    assert_eq!(hit_count(&search(&reg, "kiwi").await), 1);
    assert!(home.join("tier2/tier2.db").is_file());
    reg
}

#[tokio::test]
async fn tier2_with_providers_file() {
    let home = tempfile::tempdir().unwrap();
    let reg = assert_provider_works(home.path(), "file").await;
    // The file provider is live, so a later write is indexed in-session too.
    mem(&reg, add("memory", "Second persimmon fact")).await;
    assert_eq!(hit_count(&search(&reg, "persimmon").await), 1);
}

#[cfg(feature = "memory-sqlite")]
#[tokio::test]
async fn tier2_with_providers_sqlite() {
    let home = tempfile::tempdir().unwrap();
    assert_provider_works(home.path(), "sqlite").await;
    // Distinct files: the provider's db is never the Tier 2 db.
    assert!(home.path().join("memory.db").is_file());
    assert!(home.path().join("tier2/tier2.db").is_file());
}

//! Phase 58.2 Plan 23 (D-22, D-28): `purge_tier2` removes exactly the staged claim's MEMORY.md /
//! USER.md entry through the guarded Tier 1 path, and the claim with it.
//!
//! Every home is an explicit tempdir; embeddings point at a refused loopback port so search
//! degrades to keyword and no model is downloaded.

use std::path::Path;
use std::sync::Arc;

use ironhermes_agent::MemoryManager;
use ironhermes_agent::memory::factory::build_memory_manager_at;
use ironhermes_agent::memory::tier2_maintenance::{preview_tier2, purge_tier2};
use ironhermes_core::config::{EmbeddingsConfig, MemoryConfig};
use ironhermes_core::memory_store::MemoryTarget;
use ironhermes_core::tier2::PurgeSurface;
use tokio::sync::Mutex;

type Mgr = Arc<Mutex<MemoryManager>>;

fn cfg(tier2: bool) -> MemoryConfig {
    MemoryConfig {
        tier2_enabled: tier2,
        forgetting_enabled: true,
        embeddings: EmbeddingsConfig {
            base_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

async fn build(home: &Path, tier2: bool) -> Mgr {
    build_memory_manager_at(&cfg(tier2), home)
        .await
        .unwrap()
        .expect("memory is enabled")
}

async fn add(mgr: &Mgr, target: MemoryTarget, text: &str) {
    mgr.lock().await.add(target, text).await.unwrap();
}

/// Ids of the active claims `query` finds.
async fn ids_for(mgr: &Mgr, query: &str) -> Vec<(String, String)> {
    let t2 = mgr.lock().await.tier2().expect("tier 2 on");
    t2.search(query, 10)
        .await
        .unwrap()
        .results
        .into_iter()
        .map(|h| (h.claim_id, h.text))
        .collect()
}

async fn stage(mgr: &Mgr, query: &str, exact: &str) -> String {
    let id = ids_for(mgr, query)
        .await
        .into_iter()
        .find(|(_, t)| t == exact)
        .unwrap_or_else(|| panic!("no claim with text {exact:?}"))
        .0;
    let t2 = mgr.lock().await.tier2().unwrap();
    t2.stage_forget(&id, None).await.unwrap();
    id
}

fn read(home: &Path, name: &str) -> String {
    std::fs::read_to_string(home.join("memories").join(name)).unwrap_or_default()
}

#[tokio::test]
async fn purge_tier1_removes_memory_md_line() {
    let home = tempfile::tempdir().unwrap();
    let mgr = build(home.path(), true).await;
    add(&mgr, MemoryTarget::Memory, "User lives in Lisbon").await;
    let id = stage(&mgr, "Lisbon", "User lives in Lisbon").await;

    let preview = preview_tier2(&mgr).await.unwrap();
    assert_eq!(preview.claims.len(), 1);
    assert_eq!(
        preview.claims[0].tier1_entries,
        vec![("memory".to_string(), "User lives in Lisbon".to_string())]
    );

    let report = purge_tier2(&mgr, PurgeSurface::Cli, std::slice::from_ref(&id))
        .await
        .unwrap();
    assert_eq!(report.purged, vec![id]);
    assert_eq!(report.tier1_lines_removed, 1);
    assert!(!read(home.path(), "MEMORY.md").contains("Lisbon"));
    let prompt = mgr
        .lock()
        .await
        .format_for_system_prompt(MemoryTarget::Memory)
        .await
        .unwrap_or_default();
    assert!(!prompt.contains("Lisbon"), "{prompt}");

    drop(mgr);
    let again = build(home.path(), true).await;
    assert!(ids_for(&again, "Lisbon").await.is_empty(), "not re-indexed");
}

#[tokio::test]
async fn purge_tier1_user_md_line() {
    let home = tempfile::tempdir().unwrap();
    let mgr = build(home.path(), true).await;
    add(&mgr, MemoryTarget::User, "Prefers terse answers").await;
    let id = stage(&mgr, "terse", "Prefers terse answers").await;

    let preview = preview_tier2(&mgr).await.unwrap();
    assert_eq!(
        preview.claims[0].tier1_entries,
        vec![("user".to_string(), "Prefers terse answers".to_string())]
    );
    let report = purge_tier2(&mgr, PurgeSurface::Cli, &[id]).await.unwrap();
    assert_eq!(report.tier1_lines_removed, 1);
    assert!(!read(home.path(), "USER.md").contains("terse"));
    assert!(ids_for(&mgr, "terse").await.is_empty());
}

#[tokio::test]
async fn purge_tier1_exact_entry_only() {
    let home = tempfile::tempdir().unwrap();
    let mgr = build(home.path(), true).await;
    add(&mgr, MemoryTarget::Memory, "likes tea").await;
    add(&mgr, MemoryTarget::Memory, "likes tea with lemon").await;
    let id = stage(&mgr, "likes tea", "likes tea").await;

    let report = purge_tier2(&mgr, PurgeSurface::Cli, &[id]).await.unwrap();
    assert_eq!(report.tier1_lines_removed, 1);
    let file = read(home.path(), "MEMORY.md");
    assert!(file.contains("likes tea with lemon"), "{file}");
    assert!(!file.lines().any(|l| l.trim() == "likes tea"), "{file}");
    assert_eq!(
        ids_for(&mgr, "lemon").await.len(),
        1,
        "sibling still indexed"
    );
}

#[tokio::test]
async fn purge_tier1_already_gone_is_ok() {
    let home = tempfile::tempdir().unwrap();
    let mgr = build(home.path(), true).await;
    add(&mgr, MemoryTarget::Memory, "User lives in Lisbon").await;
    let id = stage(&mgr, "Lisbon", "User lives in Lisbon").await;
    // The operator edits the file by hand after staging; the manager's snapshot is now stale.
    std::fs::write(home.path().join("memories/MEMORY.md"), "").unwrap();

    let report = purge_tier2(&mgr, PurgeSurface::Cli, std::slice::from_ref(&id))
        .await
        .unwrap();
    assert_eq!(report.tier1_lines_removed, 0);
    assert_eq!(report.purged, vec![id]);
    assert!(report.skipped.is_none());
}

#[tokio::test]
async fn purge_moved_claim_retracts_live_line() {
    let home = tempfile::tempdir().unwrap();
    let mgr = build(home.path(), true).await;
    add(&mgr, MemoryTarget::Memory, "The mascot is a heron").await;
    mgr.lock()
        .await
        .remove(MemoryTarget::Memory, "heron")
        .await
        .unwrap();
    // D-29: the removed line moved into the daily live log and is still searchable.
    let id = stage(&mgr, "heron", "The mascot is a heron").await;

    let preview = preview_tier2(&mgr).await.unwrap();
    assert!(preview.claims[0].tier1_entries.is_empty());
    assert_eq!(preview.claims[0].live_lines, 1);

    let report = purge_tier2(&mgr, PurgeSurface::Cli, &[id]).await.unwrap();
    assert_eq!(report.tier1_lines_removed, 0);
    assert_eq!(report.live_lines_retracted, 1);
    let live: String = std::fs::read_dir(home.path().join("tier2/live"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    assert!(live.contains("<!-- retracted -->"), "{live}");
    assert!(!live.contains("heron"), "{live}");
}

#[tokio::test]
async fn purge_tier2_without_tier2_is_skipped() {
    let home = tempfile::tempdir().unwrap();
    let mgr = build(home.path(), false).await;
    let preview = preview_tier2(&mgr).await.unwrap();
    assert!(preview.skipped.is_some());
    assert!(preview.claims.is_empty());
    let report = purge_tier2(&mgr, PurgeSurface::Cli, &[]).await.unwrap();
    assert!(report.skipped.is_some());
    assert!(report.purged.is_empty());
}
